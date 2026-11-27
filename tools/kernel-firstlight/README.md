# Modern-kernel first light on the Nest J49 (via kexec)

Boot a mainline Linux kernel on the real hardware **hands-free and reversibly**,
by `kexec`-ing it from the running stock 2.6.37. No flashing, no UART: a bad
kernel is undone by a power-cycle (NAND still boots stock). Builds on
[[nest-v2-device-profile]] and tools/kexec (the kexec staging channel).

## The visibility problem — and the fix

The board has **no wired debug UART** and **no mainline display driver**, so a
freshly kexec'd kernel would boot blind. But the device hangs off the host over
**USB**, and the SoC's MUSB controller + TWL4030 USB PHY are well supported in
mainline. So the kernel is built with a **USB-gadget serial console** and the
initramfs runs a shell on `/dev/ttyGS0`. When it boots, the host sees a new
CDC-ACM device (`/dev/ttyACMx`) replacing the stock gadget (`2464:0002`) — a
full, hands-free console over the existing cable. `ramoops` (reserved RAM in the
DTB) is a secondary early-boot log channel.

## What's here

- `../../meta-j49/recipes-kernel/linux/files/omap3-nest-j49.dts` — minimal DT:
  OMAP3630, 64 MB RAM, TWL4030 (USB PHY + regulators), MUSB in peripheral mode,
  ramoops. The authoritative board DT (Phase 2 deliverable).
- kernel patches, the `j49.cfg` kconfig fragment (on `omap2plus_defconfig`:
  MUSB + g_serial + U_SERIAL console, pstore/ramoops, WiFi, DRM panel, all
  built-in, module signing off) and the DT now live in the Yocto layer:
  `meta-j49/recipes-kernel/linux/files/` (recipe `linux-j49_6.6.bb`).
  `build.sh` reads them from there, so a Yocto kernel and a first-light kernel
  are built from the same sources. References to `patches/0001` etc. below
  mean those files.
- `initramfs-init` — PID 1: mount, banner to kmsg, bring up `/dev/ttyGS0`,
  respawn a shell on it.
- `build.sh <workdir> [kver]` — fetch kernel, add+register the DT, build zImage
  + DTB (nixpkgs cross gcc), pack a static-busybox initramfs.
- `stage.sh <host> <workdir> [--exec]` — push the three artifacts; default does
  the **non-destructive** proof (`kexec -l` then `-u`), `--exec` does the live
  jump (`kexec -e`).
- `watch-usb.sh [seconds]` — host-side: detect the new serial gadget and attach.

## Build

```sh
tools/kernel-firstlight/build.sh /path/to/workdir 6.6.64
# -> workdir/out/arch/arm/boot/{zImage, dts/ti/omap/omap3-nest-j49.dtb}
#    workdir/initramfs.cpio.gz
```

Build note: `certs/extract-cert` needs openssl **host** headers/libs; pass them
via `HOSTCFLAGS=-I…/include` and `HOSTLDFLAGS=-L…/lib` (not
`KBUILD_HOSTLDFLAGS` — the top Makefile resets it). build.sh handles this.

## Stage (non-destructive) — PROVEN 2026-10-26

```sh
tools/kernel-firstlight/stage.sh 192.168.1.50 /path/to/workdir
```

Result: `kexec -l` (zImage + DTB + initrd, cmdline
`console=ttyGS0 loglevel=8 ignore_loglevel rdinit=/init`) rc=0,
`/sys/kernel/kexec_loaded` 0→1, `kexec -u` → 0. The full modern boot set is
accepted by kexec on the real silicon. Needs a stable shell — `nlsleepd` is
paused/restored around the probe.

## Hands-free live jump via log files (no USB, no physical presence)

When the device is **not** cabled to the host and nobody can power-cycle, use the
ramoops + watchdog loop instead of the USB console. The idea the whole flow is
built around:

1. **Watchdog = auto-recovery.** Stock runs `watchdogd` petting the OMAP
   hardware watchdog (`CONFIG_OMAP_WATCHDOG=y`, `NOWAYOUT`). The moment we
   `kexec -e` away, nothing pets it, so ~60 s later it **warm-resets to stock**,
   which reconnects WiFi on its own. A hung modern kernel heals itself.
   *Validated 2026-10-26:* forcing the WDT reset repeatedly brought stock back
   on WiFi every time (though a post-reset sleep trough can be up to ~14 min —
   don't mistake a long trough for a hang).
2. **ramoops = the log.** The modern kernel writes every printk to a pstore
   ram-console at `0x83f00000` (ECC off → plain text). *Validated:* DRAM
   survives the warm reset (a marker at 0x83f00000 was intact afterwards).
3. **Placement.** ramoops is a **console-only 256 KB zone at 0x83f00000** (DT).
   Stock reserves 2 MB for VRAM at the top of RAM (`Reserving 2097152 bytes
   SDRAM for VRAM` => 0x83e00000..0x84000000), and 0x83f00000 sits *inside* that
   window — which is why it survives: stock never reallocates VRAM pages, so
   (panel asleep) the region is untouched across the warm reset. No
   `mem=`/`fw_setenv` reservation is needed; `set-memreserve.sh` is kept only as
   a fallback. (A u-boot env write is a brick risk on an unreachable device —
   avoid it.)
4. **Read it back from stock** over `/dev/mem` with the cross-built reader.

### REQUIRED: appended DTB (the stock host is 2.6.37, ATAG-era)

The stock kernel is **2.6.37 and boots via ATAGs** — too old to pass a DTB to the
new kernel through `kexec` (`r2=dtb`). The first real jump (2026-10-26) proved
this the hard way: `kexec -l --dtb=…` staged fine (`kexec_loaded=1`) and the
watchdog recovered the device, but the modern kernel booted with **no device
tree**, died in `setup_arch` before ramoops init, and left **no log anywhere**
(a 16 MB RAM scan found no `DBGC` signature). So:

- Build with `CONFIG_ARM_APPENDED_DTB=y` (already in `j49.cfg`), **concatenate**
  the DTB onto the zImage (`cat zImage …/omap3-nest-j49.dtb > zImage-dtb`), and
  `kexec -l zImage-dtb --type=zImage --atags --command-line="…"` **without**
  `--dtb`. The decompressor finds the appended DTB itself, so the broken `r2`
  handoff is irrelevant; `CONFIG_ARM_ATAG_DTB_COMPAT=y` folds the atag cmdline/mem
  into the appended DTB. **`--atags` is required**: with neither `--atags` nor
  `--dtb`, kexec-tools falls back to scanning `/proc/device-tree/`, which an
  ATAG-booted 2.6.37 kernel does not have (`unrecoverable error: could not scan
  "/proc/device-tree/"`).
- **Marker pre-seed = signal-of-life.** Before the jump, write a known pattern
  (0xA5A5A5A5) across the ramoops region from stock (`devmem-arm`). After
  recovery, classify it: `DBGC` header => kernel reached ramoops init (log
  captured); zeroed => kernel entered but ramoops didn't register; pattern
  intact => kernel never touched that RAM (didn't enter / died in the
  decompressor). This makes every jump diagnostic even when it boots blind.

```sh
tools/kernel-firstlight/build-reader.sh /path/to/workdir        # -> workdir/ramoops-read

# the jump (SSH drops; watchdog brings stock back, then a wake window up to ~14 min):
tools/kernel-firstlight/logjump.sh 192.168.1.50 /path/to/workdir

# after stock is reachable again, recover the modern kernel's boot log:
tools/kernel-firstlight/read-ramoops.sh 192.168.1.50 /path/to/workdir
```

The init writes `NEST-FL: STAGE1/2/3` markers, so the recovered log shows exactly
how far the modern kernel got (userspace reached, USB gadget bound, etc.).

**Operational note:** the device sleeps aggressively (`nlsleepd`); the FIRST
action on any wake window must be `kill -STOP nlsleepd` (and `monit`) to hold a
stable shell — otherwise multi-step pushes race the sleep and fail.

**Residual risk:** if a warm reset ever comes back *unclean* (not seen in
testing — resets recovered every time, but the earlier *interrupted*-reboot
outage shows it's not zero), the device is offline until a physical power-cycle.
The watchdog covers a *hung* kernel, not a bad reset. The safe-zone upper bound
(0x83f00000 safe … 0x83fc0000 clobbered) is unverified — if the first real jump
returns a partially-zeroed log, shrink the ramoops region.

## Alternative: live jump with a USB console (needs host cable + presence)

If the device *is* cabled to the host and someone can power-cycle, skip the
ramoops dance and watch the g_serial gadget live:

```sh
# terminal 1 (host):
tools/kernel-firstlight/watch-usb.sh 180
# terminal 2:
tools/kernel-firstlight/stage.sh 192.168.1.50 /path/to/workdir --exec
```

Success = a new `/dev/ttyACMx` with a shell banner on the host. Failure = nothing
comes back → power-cycle (stock is intact).

## PROVEN hands-free path (2026-10-27, first light achieved)

Eight kexec jumps in one day, every one recovered without touching the device.
What finally worked, and why each piece is there:

1. **Stock prep (once):** `mem=62M` in the u-boot `addmodel` env var (stock keeps
   out of 0x83e00000+), `nlsleepd` disabled via `/etc/monit.d/nlscpm.monitrc`
   (`#DISABLED#` lines, `.orig` backup, `kill -HUP monit`) so the device is always
   reachable. `/media/scratch` is wiped at every boot — re-push each cycle.
2. **Post-mortem window = 0x83e00000..0x83f00000.** Above stock's RAM top, below
   U-Boot's footprint (U-Boot clears ~0x83f03000..0x84000000 every boot — the
   original ramoops@0x83f00000 was inside that). ramoops @0x83e00000 (256 KB),
   DEBUG_LL DRAM ring @0x83e80000 (64 KB, `patches/0001-...`).
3. **DEBUG_LL -> DRAM ring** (`arch/arm/include/debug/ramring.S`): decompressor
   banner, head.S breadcrumbs A-D, `earlyprintk keep_bootcon`, `NEST_MARK()`s
   through paging_init. Ring virt lives in vmalloc space (0xf7e80000) and is
   re-mapped by `debug_ll_io_init()` from `omap3_map_io()` — a linear-map virt
   died at devicemaps_init's TLB flush (jumps 6/7).
4. **kexec from 2.6.37:** appended DTB + `--atags` + `mem=64M`, **and** the zImage
   header `_magic_end` bumped (`zImage-dtb-kx`) because kexec-tools otherwise
   strips the appended DTB (jumps 2-4 died with "invalid dtb ... machine ID").
   Stock's `cpu_v7_reset` is `mov pc, r0` (MMU stays on); that is fine.
5. **Recovery:** the stock-armed OMAP watchdog does NOT save you (kexec's
   `device_shutdown()` disables it). A kernel that boots must reboot itself:
   `initramfs-init` warm-reboots after 90 s unless a USB host is attached
   (`panic=15` on the cmdline). A hung kernel is reset by an autonomous
   ~29-minute cold reset (PMIC/backplate) which still preserves DRAM.
6. **Read back:** `memdump` (mmap /dev/mem; stock busybox has no devmem) +
   `decode-postmortem.py`. Full first-light logs in `logs/`.

Loop: `build.sh` -> `realjump3.sh` -> `recover3.sh` (~5 min when the kernel boots).
Open items from the first-light log: TWL4030 I2C errors from ~5 s
(`twl: Write failed (mod 0, reg 0xfd)`, VUSB regulators -EREMOTEIO) in the
runtime-PM path; `48318000.target-module` probe -EBUSY; hsmmc RX DMA.

## The TWL4030 I2C problem — SOLVED (2026-10-28): `sys_clkout1` feeds the PMIC

`clk_disable_unused()` (late_initcall, 3.53 s in our boot) gates exactly one
clock on this board: **`sys_clkout1`** (PRM_CLKOUT_CTRL 0x48306d70 bit 7, left
on by U-Boot/stock). SYS_CLKOUT1 (19.2 MHz) is the TWL4030/TPS65921 HFCLKIN on
the j49, so gating it stops the PMIC's I2C slave. From then on
every TWL4030 access fails (NACK, "Arbitration lost", "controller timed out",
finally "SCL is stuck low"), the USB PHY never sees VBUS and MUSB never answers
SETUP. `clk_ignore_unused` on the command line proved it (jumps 18-23: all four TWL
slaves read 100 % for the whole run); `patches/0003-omap3-keep-sys_clkout1-nest-j49.patch`
is the proper fix (sys_clkout1 joins TI's `enable_init_clks[]`; verified in
jump 26 with the flag removed: 22/22 probe rounds, zero TWL errors). Found with a
one-line debug print in `clk_disable_unused_subtree()` after register snapshots
with and without the flag came out identical (`unused-victims.py`, `diff-rawregs.py`).

### How it was found (the wrong turns are worth keeping)

Jumps 12 and 14 fail identically, so the earlier "deep cpuidle" theory is dead
(jump 14 ran C1-only, `patches/0002-...`, with i2c1 `ti,no-idle` and runtime PM
pinned on, and still logged the exact same two episodes):

| time   | what reads                                        | module (TWL slave)       |
|--------|---------------------------------------------------|--------------------------|
| 4.4 s  | `twl4030_usb_irq` -> linkstat, STS_HW_CONDITIONS  | PM_MASTER (0x4b)         |
| 33.8 s | `regulator_init_complete` DEV_GRP sweep 0x17..0x3b | PM_RECEIVER (0x4b)       |

Everything on 0x48/0x49 keeps working, the power-SIH init at 1.8 s and the
regulator reads at the PHY power-on (~3.3 s) still read 0x4b fine, and stock
reads all four TWL addresses 100 % (`i2cprobe`, same 2-message read regmap
uses). PRM VP1/VP2 are disabled under stock and VOLTCTRL=0, so the SR-I2C
collision idea is out too. The window: 0x4b works at ~3.3 s, is dead at 4.4 s;
in between the USB PHY LDOs are enabled and the PHY + its DPLL are powered —
and, decisively, `clk: Disabling unused clocks` is logged at 3.53 s. Jump 17
(usb_otg_hs disabled) died the same way, so the PHY was never the trigger.

Instrumentation now in `initramfs-init`: `i2cprobe /dev/i2c-0 scan` once, one
`I2C ...` line per 2 s tick (0x48/0x49/0x4a/0x4b), PRM VC/VP dump, and an
experiment that unbinds musb + twl4030_usb at 44 s to see whether 0x4b
recovers when the PHY is off. `kbuild/build-dtb-variant.sh nousb` builds the
follow-up (usb_otg_hs disabled from the start).

Tooling notes: `recover5.sh` dumps on-device at first contact and pulls only the needed 64 KiB
chunks (a 1 MiB SSH stream over this WiFi truncates); `chain15.sh N` runs
jump -> recover -> decode unattended. Never `memdump` an unclocked peripheral
on stock (SR1 @0x480c9000 -> external abort -> `panic=1` -> reboot).

### After the fix (jump 27)
Deep cpuidle works with `sys_clkout1` kept on: C3 (MPU RET) and C5 (MPU+CORE
RET) are entered hundreds of times per run with zero I2C errors, so the earlier
C1-only patch is gone. `initramfs-init` now leaves all C-states and runtime PM
enabled but keeps runtime PM pinned on for I2C1 and the TWL USB PHY: jump 28,
which left runtime PM at its default together with deep idle, hung hard (no WDT
reset, 29-min cold reset, DRAM lost). Jump 29 confirmed the default (C3/C5 used,
22/22 TWL probes). Command-line tokens: `nest_fl_rpm` leaves runtime PM at its
default (to re-test that hang), `nest_fl_c1only` re-applies the old C1-only
workaround.
Kernel = 6.6.64 + `patches/0001` (DRAM ring) + `patches/0003` (sys_clkout1).

## USB console, off-base (2026-10-28, first interactive 6.6 session)
The display can be undocked and cabled to the host while stock is running: it
enumerates as mass storage (`2464:0002`) and *still* opens its WiFi/SSH window,
so the normal `realjump3.sh` works over WiFi with the cable already in. About
20 s after `kexec -e` the host sees `0525:a4a7 "Linux 6.6.64 with musb-hdrc"`
and a `/dev/ttyACMx`; `/init` runs a shell on it (STAGE4) and `/stage4` cancels
the 90 s auto-reboot. `tty.py DEV SECONDS cmd...` drives the port without
pyserial; `usbjump.sh` (wait for stock → optional `stockprobe.sh` → jump → wait
for the console → snapshot) is the off-base iteration loop; `reboot -f` on the
console brings stock back on USB and the next window follows ~2.5 min later.

What the first session showed:
- **hwmod init softresets all six GPIO banks** (`OE=0xFFFFFFFF`, `DATAOUT=0`),
  so every output stock had driven is gone: the I2C2/I2C3 sensors (ring
  `0x57`, `0x36`) do not answer, LCD/backlight and the backplate reset line are
  presumably off too. Fix: `&gpioN { ti,no-reset-on-init; }` for gpio1..6 in
  the DTS (same mechanism as the i2c1 override). Turning VAUX1..4 back on did
  not revive the sensors by itself. `stockprobe.sh` captures stock's GPIO bank
  registers, pad configuration and the 2.6.37 debugfs owner table
  (`/sys/kernel/debug/gpio`, `omap_mux/board`) as the reference.
- **Battery is safe off-base without a charger driver**: TWL `BCIMSTATEC=0x1c`
  = USB charging mode, state COMPLETE2 (full); the hardware BCI keeps charging.
  `CONFIG_CHARGER_TWL4030`, `TWL4030_MADC` and `RTC_DRV_TWL4030` were `=m`
  (useless without modules) and are now built in via `j49.cfg`.
- UARTs come up as `ttyS0..3` (8250-omap); stock's `ttyO2` (backplate STM32,
  UART3 @ 0x49020000) is **`ttyS2`** here. The backplate is only reachable on
  the base, so the protocol work needs the on-base path.
- TWL RTC seconds stay at 0 although RTC_CTRL/RTC_STATUS say "running" (open).
- `memdump` of the whole SCM block (0x48002000) bus-errors under 6.6; single
  `devmem` reads of pad registers work, and
  `/sys/kernel/debug/pinctrl/48002030.pinmux-pinctrl-single/pins` lists them.
- `regulator_init_complete` switched VAUX1-4/VMMC1-2/VPLL2/VSIM/VDAC off at 30 s
  (no consumers); whether stock had them on is part of the stockprobe output.

### GPIO fix verified, TWL driver freeze bisected (jumps 30-32)
The kernel with `ti,no-reset-on-init` on gpio1..6 *and* the charger/MADC/RTC
drivers built in froze hard at 5.15 s (jump 30: ramoops and the DEBUG_LL ring
both stop mid-STAGE2, no oops, watchdog reset a minute later). Jump 31, same
kernel with `initcall_blacklist=twl4030_bci_driver_init,twl4030_madc_driver_init,twl4030rtc_driver_init`,
ran clean (STAGE5 at 106 s, 22/22 TWL probes, C3/C5 used): the GPIO change is
innocent, one of the three TWL drivers stops the SoC. `initcall_blacklist` with
the `<driver>_driver_init` name of a `module_platform_driver()` built-in is the
cheap way to bisect without rebuilding. STAGE2 now also logs the six GPIO banks
(`STAGE2 GPIO`) and scans I2C2/I2C3 (`STAGE2 I2Cbus`) so the on-base loop can
verify the sensors come back.

Stock quirk learned the hard way: after a *watchdog* reset, stock off-base (on
USB) shows "please attach display to its base" and opened no WiFi window in 13
minutes, while after a clean `reboot -f` it did. Off-base iteration therefore
only works while the 6.6 kernel exits cleanly; a frozen kernel means docking
the display again.

Jump 32 (charger blacklisted, MADC + RTC on) ran clean too, so **`twl4030_bci`
(CONFIG_CHARGER_TWL4030) is the freezer**; it is now off in `j49.cfg` (the
hardware BCI charges over USB by itself, TWL `BCIMSTATEC=0x1c` = USB/complete).
Why the driver stops the SoC within ~1.5 s of probing is still open. With the
GPIO fix the banks match stock, except gpio103 (WiFi power) and gpio177 (lm3530
backlight enable), which arrive low: stock's `kexec -e` runs `device_shutdown()`
and its drivers switch those off first, so 6.6 must re-assert them. The ring
sensor (0x57 on I2C2) answers again; 0x36 on I2C3 is still missing at 4.7 s.
`twl_rtc` keeps time across jumps (raw `SECONDS_REG` reads over I2C return a
stale value, which is why the RTC looked stopped). The stock GPIO owner table
captured by `stockprobe.sh` lives in `reference/stock-gpio-owners.txt`.

### USB session 3 (2026-10-28 15:56): rails, sensors, first WiFi contact
- The unknown `0x36` on I2C3 is powered by **VAUX1** (3.0 V) alone (DTS now
  marks `vaux1` always-on). It reads 0xff everywhere except two registers, so
  its identity is still open.
- Ring sensor confirmed: Avago **ADBS-A330** at I2C2 `0x57` (Product_ID 0x88).
- MADC works (`in_voltage8` = VBUS 5.1 V); the VBAT channel needs BCI
  monitoring enabled and reads garbage without it.
- **WiFi:** driving gpio103 high (stock's "vmmc2" GPIO) and rebinding
  `480b4000.mmc` made the **WL1271 answer on MMC2** ("card claims to support
  voltages below defined range" = missing `vmmc-supply`). The DTS now has a
  fixed `wlan_en` regulator on gpio103, `mmc2` with `non-removable` +
  `cap-power-off-card` and a `ti,wl1271` child (irq gpio102, 19.2 MHz ref
  clock, to be confirmed); `j49.cfg` builds cfg80211/mac80211/wlcore in and
  `pack-initramfs.sh` ships `wl127x-fw-5-*.bin`. The device NVS lives in
  stock's `/media/system-config/wl12xx-nvs.bin` (mtd10, little-endian jffs2);
  it is device-specific and stays out of git (`grabnvs.sh` fetches it).
- lm3530 backlight controller: gpio177 is its HWEN, but no `0x38` showed up on
  I2C2/I2C3 with it high; bus/address still unknown.
- The STAGE4 USB session ends after `NEST_FL_USB_MAX` (1200 s) with a reboot.

### Jump 34: WL1271 detected on 6.6
With the `mmc2`/`ti,wl1271` node the on-base run logged `mmc1: new SDIO card
at address 0001` and `wlcore: loaded` (clean 90 s run otherwise). Two details
the mainline driver wants: the NVS under the name
`ti-connectivity/wl127x-nvs.bin`, and `regulatory.db` + `.p7s` from
wireless-regdb in `/lib/firmware`; the firmware itself only boots when the
interface comes up, so `initramfs-init` now runs `ip link set wlan0 up` and
logs the result as `STAGE2 WIFI`. The device NVS comes out of the NAS dump of
mtd10: a little-endian jffs2 image that starts after 0x1a0000 of erased
flash (trim it first, jefferson otherwise guesses big-endian). Alongside it
the partition holds the stock ssh host keys, so nothing from it goes into git.

### Jump 35: WiFi firmware boots
`wlcore: firmware booted (Rev 6.3.10.0.142)` with the device NVS and the
regulatory database in place; the run stayed clean. Next for WiFi: a
wpa_supplicant in the initramfs and the WPA passphrase supplied at jump time,
never from the repo. The I2C3 `0x36` device is most likely the **lm3530
backlight controller** itself: it answered only with gpio177 (its HWEN) high
and VAUX1 on, and the registers that read back (0x10, 0x20) match its map.
`nest_fl_bl` on the command line makes `initramfs-init` raise gpio177, switch
the lm3530 to I2C brightness mode and set half brightness (logged as
`STAGE2 BL`): a visible test that needs someone looking at the panel.

### Jump 36: lm3530 confirmed, wlan0 up
With gpio177 high the `0x36` device appears and accepts the lm3530 writes
(GEN_CONFIG 0x10 -> 0x11, BRT_CTRL 0xA0 -> 0x40 read back 0x11/0xc0). wlan0
comes UP with the device MAC from the NVS. Visual confirmation of the
backlight is still pending (nobody was near the panel).

### Jumps 37-39: network userland in the initramfs (wpa_supplicant, DHCP, SSH)
The initramfs now carries stock's armhf wpa_supplicant (v2.0-Nest_2017a), `iw`
and dropbear 2014.63 with their glibc 2.19 shared-library closure (staged by
the scratchpad pack script, 4.2 MB compressed). Secrets stay out of git:
`/etc/wpa.conf` (SSID + derived PSK), the device's own dropbear host keys and
`/root/.ssh/authorized_keys` (host RSA key; that dropbear has no ed25519) are
staged from the scratchpad. Dropbear runs pubkey-only (`-s`) so the stock
recovery scripts' root/admin probe keeps failing on 6.6 and cannot mistake it
for stock. Once dropbear is up `initramfs-init` touches `/netup`: the main
loop then holds the device up to `NEST_FL_NET_MAX` (default 900 s; touching
`/hold` over ssh raises it to `NEST_FL_HOLD_MAX`, 3600 s), logging every 30 s.

- **Jump 37 died of memory, not WiFi.** `Memory: 22968K/63488K available`:
  16 MB CMA plus the 9 MB tmpfs rootfs left ~9 MB of non-CMA memory, below
  the boosted watermark, and the order-2 allocation in
  `wlcore_boot_upload_firmware` triggered the OOM killer at 8.9 s. It killed
  init subshells including the backstop's `sleep`, so the backstop fired at
  9 s; `reboot -f` could not fork and the HW watchdog ended the run at 67 s.
  Fixes: `cma=0` on the command line (no DSS yet), the backstop and USB-cap
  timers poll `/proc/uptime` instead of trusting one `sleep`, and a
  `STAGE2 mem:` line reports MemFree/MemAvailable/Shmem.
- **Jump 38:** 39384K available, 21.7 MB free after unpacking, clean run to
  STAGE5. wpa_supplicant started (`Successfully initialized`) but never
  associated within 45 s and logged nothing more. Jump 39 adds `iw reg get`,
  a kernel-side `iw dev wlan0 scan` summary and `wpa_supplicant -d` with a
  filtered log tail to see whether the old supplicant even scans on 6.6.
- **Jump 39** (static wpa_supplicant 2.12 from
  `pkgsCross.armv7l-hf-multiplatform.pkgsStatic`, stock v2.0 and its
  libcrypto/dbus dropped): the kernel-side `iw dev wlan0 scan` returned **0
  BSS**, and dmesg shows why: `wlcore: WARNING ELP wakeup timeout!`,
  `cmd set_template failed: -5`, `PROBE request template failed`, then a
  wlcore recovery (`wlcore: down`, firmware rebooted). The ELP wakeup is the
  first operation that needs the chip's interrupt (firmware boot only polls),
  so the prime suspect is the WL1271 IRQ on gpio102 not being delivered.
  Stock: `gpio-102 (WL127x IRQ) in lo irq-262 level-high wakeup`, pad
  `cam_d3.gpio_102` = 0x771c (mode 4, input, pull-up, off-wakeup); the 6.6
  pad reads 0x371c (same minus OFF_WAKEUPENABLE). Jump 40 logs the wl1271
  line of `/proc/interrupts` before and after the scan.

### Display (jump 40+): omapdrm + Samsung S6D05A1 panel driver
The panel is a Tianma TM025ZDZ01 320x320 module driven by a Samsung S6D05A1:
9-bit SPI control on McSPI1 CS0 (mode 3, <= 10 MHz) and 24-bit parallel RGB
from the DSS. Everything needed is in the vendor 2.6.37 tree
(`~/Projects/linux`, cuckoo-nest): `board-diamond.c` (DVT data = j49: McSPI1,
reset gpio109 active low, lcd_id gpio105, VCC "lcd2v8" = TWL VAUX2 2.8 V,
DPI 24 lines, `OMAP_DSS_LCD_TFT|IPC|IEO|IVS|IHS`) and
`drivers/video/omap2/displays/panel-tianma-tm025zdz01.c` (timings 320x320 @
7.18 MHz, hsw 2 / hfp 16 / hbp 16, vsw 2 / vfp 8 / vbp 8, and the full
level-2 register sequence). `patches/0004-drm-panel-samsung-s6d05a1-nest-j49.patch`
adds `drivers/gpu/drm/panel/panel-samsung-s6d05a1.c` (also kept as
`meta-j49/recipes-kernel/linux/files/panel-samsung-s6d05a1.c`): a DRM DPI
panel that replays the vendor sequence with the parameters pre-encoded,
rotated 180 degrees like stock (MADCTL 0xd0). The S6D05A1's DC/DC booster
runs from the external pixel clock, so the command sequence goes out from
`.enable` (CRTC already running), mirroring the vendor order
(`omapdss_dpi_display_enable()` before `s6d05a1_power_on()`). Bus flags:
DE low, pixel data on the falling clock edge (vendor IEO/IPC).

The DTS gained the DSS DPI and McSPI1 pad groups (stock mux values), `&dss`
with `vdds_dsi-supply = <&vpll2>` and a 24-line DPI endpoint, `&vaux2` at
2.8 V, and the panel on `&mcspi1` with `reset-gpios = <&gpio4 13 GPIO_ACTIVE_LOW>`.
`j49.cfg` turns on `DRM`, `DRM_OMAP`, `OMAP2_DSS_DPI`, the panel, fbdev
emulation, fbcon and the boot logo (VENC/SDI/DSI/HDMI off). Jump 40 boots
with `console=tty0 cma=4M`: if the pipeline works the kernel log scrolls on
the panel, and `initramfs-init` additionally writes noise to `/dev/fb0`, clears
it and prints a banner on `/dev/tty0` (logged as `STAGE2 FB`).

Backlight: the user confirmed visually during jump 39 that the panel lights
up with `nest_fl_bl` (lm3530 via I2C3 0x36, HWEN gpio177) -- "only backlight",
as expected without a panel driver.

### Jumps 40-41: first picture on the LCD
- **Jump 40** panicked at 2.7 s: `System is deadlocked on memory`. With the
  DRM kernel (13.3 MB code) `cma=4M` still reserved 8 MB and the 6.8 MB
  initramfs (7.9 MB static wpa_supplicant inside) left nothing for the
  unpacked rootfs: `Memory: 27640K/63488K available`. The panel driver had
  already probed fine (`S6D05A1 panel registered`), omapdrm initialised.
- **Jump 41** (same kernel, `cma=0`, display-only 1.3 MB initramfs):
  `S6D05A1 display on, ID 68 45 01`, `Console: switching to colour frame
  buffer device 40x40`, `fb0: omapdrmdrmfb`; init reports
  `virtual_size=320,320 bits_per_pixel=32 stride=1280`, writes noise, clears
  and prints a banner on tty0. 41 MB available, 26 MB free with the rootfs
  unpacked. The scanout buffer comes from the non-CMA allocator without
  trouble. The loop/backstop timing mismatch seen here (backstop at 185 s
  before the loop reached GRACE) is fixed by counting `t` from /proc/uptime.
- The static wpa_supplicant is now a purpose-built 0.86 MB binary
  (`build-wpa-min.sh`: hostap 2.12, nl80211 + WPA2-PSK + internal crypto, no
  EAP/SAE/dbus, static musl via nixpkgs pkgsStatic libnl), so network and
  display fit together (jump 42).

Keeping stock awake between jumps: `nlscpm` (inside `/nestlabs/sbin/nlsleepd`)
suspends via `/sys/power/state` when no DBus sleep client objects; `kill -9`
is useless (monit restarts it) and 2.6.37 has no wakelocks here. What works:
`mount --bind /dev/null /sys/power/state` on stock (realjump3 does it in its
first command, `holdawake.sh` repeats it) -- the suspend write fails and the
device stays on WiFi until its next reboot. Wake windows otherwise: 2-7 min
after boot, then ~25 s every ~38 min.

### Jumps 42-45: the WiFi root cause was the reference clock
- Jump 42 (display + network together, 22 MB free): clean, but still
  `ELP wakeup timeout` and `iw scan` = 0 BSS. `/proc/interrupts`: the WL1271
  line (gpio4/6, level) fired exactly 2 times per firmware boot and never for
  the ELP wake-up.
- Jump 43 (`nest_fl_c1only`, deep idle off): identical. Jump 44 (IRQ as
  `IRQ_TYPE_EDGE_RISING`): identical (1 IRQ per boot). So neither idle nor the
  trigger type.
- Stock `board-diamond.c` `display_2_0_net_init()` (the Display-2.x models =
  j49) passes **`WL12XX_REFCLOCK_26_XTAL`**; the DTS said 19.2 MHz. With the
  wrong reference the firmware still reported "booted" but the chip was
  effectively deaf.
- Jump 45 (`ref-clock-frequency = <26000000>`): scan returns 9 BSS, ours at
  -51 dBm, the IRQ line fires 36 times, no ELP timeout. `clock-xtal` is only
  parsed by `wlcore/spi.c` in 6.6, so `patches/0005-wlcore-sdio-parse-clock-xtal.patch`
  adds the same `of_property_read_bool(np, "clock-xtal")` to `wlcore/sdio.c`.
  Association did not happen within 60 s and the reason was invisible: the
  minimal supplicant lacked `CONFIG_DEBUG_FILE`, so `-f /wpa.log` was ignored.
  Jump 46 fixes that and logs the AP's RSN IE plus `wpa_cli status`.

### Jump 46: association, DHCP and the dropbear crash
- With the XTAL flag and the logging supplicant: `Association completed` 3 s
  after the supplicant started, 4-way + group handshake done at 26 s, DHCP
  lease at 26.3 s (the AP is WPA/WPA2-PSK CCMP, no PMF). The first gateway
  ping right after DHCP failed (ARP not settled); the host saw the device
  answer ping later in the 900 s hold.
- No SSH though: `dropbear rc=1`. Stock's dropbearmulti 2014.63 is a *static*
  glibc 2.19 binary and that exit code is its own SIGSEGV handler ("Aiee,
  segfault!"). Under `qemu-arm` on the host it crashes identically, right after
  installing the signal handlers in `commonsetup()` (NULL+4 read inside
  `crypto_init`); `.bss` is zeroed, so it is not the 6.x ELF-loader bss change.
  It only works on 2.6.37; root cause not pursued. Replaced by
  `nix build nixpkgs#pkgsCross.armv7l-hf-multiplatform.pkgsStatic.dropbear`
  (2026.91, static musl, 0.74 MB), started with `-s -E -p 22 -R -r <stock keys>`;
  the stock dropbearmulti and its symlinks left the initramfs (3.35 MB now).
- The init logs `/dropbear.log`, the listen socket count and a delayed gateway
  ping so a failing SSH daemon is visible in ramoops next time.
- Stock stayed up for 40 min after the jump with `/sys/power/state` bound, then
  dropped off WiFi at 23:33 and was not seen again overnight (the proximity
  sensor woke the UI in the morning). Cause unknown; the bind mount does not
  survive a reboot.

### Jumps 47-48: SSH over WiFi, DISPC verified against stock
- Jump 47 (SYNC_DRIVE_POSEDGE kernel, static dropbear): association 7 s after
  the supplicant started, DHCP at 30 s, `dropbear rc=0 listen=2`, held 900 s.
  Nobody probed with the key (the watcher had timed out), so jump 48 repeated
  it with the watcher armed: **6.6 answered `ssh root@<ip> cat /proc/version`
  over WiFi 2 min after the kexec** (pubkey auth, dropbear 2026.91 generated
  an ed25519 host key next to the stock RSA/ECDSA keys). `touch /hold` then
  extended the hold to 3600 s and the rest of the session was interactive.
- Stock itself had been off WiFi for 13 h after the night's jump 46 (uptime
  49790 s when it reappeared, so no reboot happened: it simply stopped joining
  the AP). It came back on its own at 12:39.
- DISPC registers read over SSH (`devmem`; `memdump` of whole 4 KiB windows
  faults on unmapped holes: "In-band Error seen by MPU"): CONTROL 0x18309
  (LCD on, TFT, 24 data lines), TIMING_H 0x00F00F01 / TIMING_V 0x00800801
  (hsw 2, hfp 16, hbp 16, vsw 2, vfp 8, vbp 8), POL_FREQ 0x3F000 (IVS IHS IPC
  IEO, ONOFF=1 RF=1: syncs on the rising edge, data on the falling edge, which
  is what stock's ONOFF=0 + IPC=1 also produces), DIVISOR lcd 1 / pcd 4 on the
  28.8 MHz dss1_alwon_fck = 7.2 MHz pixel clock, SIZE_LCD 320x320, GFX xRGB8888
  from 0x83180000, IRQSTATUS shows VSYNC and GFX_END_WINDOW, no FIFO underflow
  or SYNC_LOST. The DSS pads 0x20d4-0x2104 are all mode 0 outputs, mcspi1 pads
  as in the DTS, pad 0x21d4 (mcspi1_cs3) is mode 4 = gpio177 (lm3530 HWEN).
  So the controller side matches stock; what remains unverified is the panel
  itself and the backlight.
- Backlight: stock's j49 lm3530 platform data is manual (I2C) mode, 29 mA full
  scale, exponential ramp, max brightness 0x78 (GEN_CONFIG 0x1d). The init had
  used 0x11 (19 mA); it now programs the stock values. The LM3530 ALS zone
  register does not react to the backlight, so no hands-free light check.
- The stock 1.8 V -> 3.3 V LCD buffer / VDD GPIOs only exist on the development
  board (`diamond_display_gpio_init` is called from the development display
  init only), not on j49.
- Next kernel (jump 49) logs the panel's RDDPM/RDDMADCTL/RDDCOLMOD/RDDIM/RDDSM/
  RDDSDR status registers after DISPON, so the panel state is visible in
  ramoops without eyes on the device.

### Jump 49: the panel says it is on
- Kernel with the status readout, SSH 2 min after the kexec, `touch /hold`.
  Because the early lines had already left the kernel ring buffer, the panel
  enable was re-run with `echo 1 > /sys/class/graphics/fb0/blank; echo 0 > ...`
  (fbdev emulation -> DRM -> panel disable/enable):
  `S6D05A1 status RDDPM=9c MADCTL=d0 COLMOD=77 IM=00 SM=00 SDR=c0`.
  RDDPM 0x9c = booster on, sleep out, normal mode, **display on**; COLMOD 0x77
  = 24 bpp; RDDSDR 0xc0 = register loading and functionality checks passed.
  Together with the DISPC state from jump 48 the whole DPI path is up. A black
  screen therefore means no backlight.
- Backlight on j49 = LM3530 at I2C3 0x36, HWEN gpio177, manual (I2C) mode.
  Stock `lm3530_init_registers()` writes GEN_CONFIG 0x1c, ALS_CONFIG 0x00,
  BRT_RAMP 0x2d, zone/impedance 0, brightness 0, zone boundaries 0, zone
  targets 7f/66/4c/33/19, then GEN_CONFIG 0x1d (I2C_EN last); userspace then
  writes brightness 0..0x78 straight into 0xA0. Our init only wrote 0x11 and
  0x7f and left the chip's power-on ALS_CONFIG 0xac (ALS enabled) in place; the
  exact stock sequence was applied live over SSH and is now in the init. Whether
  it lights the panel needs eyes on the device.
- `stockdiag.sh` dumps stock's DISPC, padconf and GPIO banks via memdump for a
  register-level diff against 6.6 the next time stock is reachable.

### Jumps 50-51: 6.6 runs on the battery and browns out after ~29 min
- Both sessions (held with `/hold`, 3600 s cap) ended with a cold reset about
  29 min after the kexec: stock logged `twl_rtc: Power up reset detected` and
  `omap_wdt: Last boot caused by Reset`, and the ramoops zone was gone (power
  loss, "kernel entered but died before ramoops probe"). Stock has no
  power_supply class and no BCI driver (`board-diamond-battery.c` is only the
  battery-disconnect GPIO 161), so charging is autonomous hardware fed by the
  backplate's power stealing and is sized for a mostly-sleeping device.
  6.6 with CPU, WiFi and backlight on draws more than that; the TWL4030 MADC
  current channel read about -750 mA (in_current10_input) while the battery
  voltage channel is not scaled usefully.
- Consequences: keep 6.6 sessions short (`NEST_FL_HOLD_MAX` now 1200 s), let
  stock sleep between jumps (the keep-awake bind mount also burns battery:
  stock very likely drops WiFi on low battery, which explains the 13 h
  absence after jump 46), and get a charger path working before long
  sessions (`CONFIG_CHARGER_TWL4030` + the `bci` node in twl4030.dtsi, to be
  checked against what the backplate actually supplies).
- MADC channel 0 (ADCIN0, ~1.22 V) drops reproducibly by 13 counts when the
  lm3530 brightness goes 0 -> 0x78 (3 of 3 rounds): the backlight driver does
  load the supply when told to. (Visual confirmation came in jump 54: the
  panel lit white then red/green/blue -- see below.)
- Panel status after the live blank/unblank in jump 49-51 is unchanged
  (RDDPM 0x9c). `/sys/class/gpio` on 6.6 numbers the banks from 512
  (gpio177 = export 689).

### Jump 52: the 29-minute reset is the backplate, not the battery
- The brown-out story above does not hold. Three facts from the day after:
  1. The PMIC is a **TPS65921**, the TWL4030 variant *without* the BCI charger
     block. That is why stock has no power_supply class, why
     `CONFIG_CHARGER_TWL4030` froze the 6.6 kernel in jump 30, and why the
     MADC "charger current" channel 10 (the -750 mA) is meaningless here.
  2. Every cold reset on record came ~29 min after the kexec regardless of
     battery state: jump 3 (fresh off the base), 17, 28, 50 and 51 (51 started
     three minutes after 50 had "drained" the battery), and jump 52.
  3. Jump 52 was run with a 2400 s hold while the host sampled the TWL MADC
     every 15 s over SSH (`vbatwatch.sh`). ADCIN0, the divider stock's
     shutdown-manager reports as `batteryLevel` (stock hwmon `in0` 1249 mV
     while it logged 3.789 V), went from 835 to 829 counts in 27 min
     (~3.70 -> 3.68 V, flat) and the device vanished at uptime ~1730 s. The
     battery-disconnect GPIO (161, sysfs 673) read 0 throughout.
- The backplate firmware in the stock rootfs (`nlbpfirmware.plist`,
  AMBER_BP app v1.0.26, Intel HEX -> 52 KB binary) contains the strings
  `reset the HU now`, `No HU, skipping reset of the HU`, `Disconnecting HU`,
  `Resetting HU due to Demo mode TO` and `** skipped watchdog`: the STM32 on
  the backplate supervises the head unit (HU) over the serial link
  (`/dev/ttyO2` = UART3, no flow control, per `nestlabs/etc/product.config`)
  and power-cycles it when the HU goes quiet. After a kexec nothing talks to
  it, so ~28.7 min later it pulls the plug (cold reset, RTC power-up flag,
  DRAM logs lost).
- Consequences: a 6.6 session is hard-limited to ~28 min until the kernel
  speaks the backplate protocol (or kicks whatever the STM32 counts as a
  heartbeat). Hold caps are now 900 s / 1500 s (`NEST_FL_NET_MAX` /
  `NEST_FL_HOLD_MAX`) so the session ends in a warm reboot with ramoops
  intact. Letting stock "charge for hours" between jumps was unnecessary; the
  13 h WiFi absence after jump 46 is stock's own power-save behaviour.
  Next steps: on 6.6 open `/dev/ttyS2` (UART3 is clocked, 8250_omap built
  in) and sniff what the backplate sends, then recover the HU->backplate
  framing (header, length, CRC: `* bad len`, `* crc mismatch`, `BRK %02x`
  in the firmware) from `nlclient` ("backplate connections manager").
- Stock measurements right after jump 52 (`stock-bp-*.txt`): `/dev/ttyO2`
  runs 115200 8N1 raw, owned by `nlclient`; `/proc/tty/driver/OMAP-SERIAL`
  grew by 286 rx and 9 tx bytes in 10 s, i.e. the backplate streams ~29 B/s
  to the head unit while the head unit sends about one byte per second (the
  heartbeat candidates). One `brk` was counted (the firmware logs `BRK %02x`).
  Calibration: stock hwmon `in0_battery_type` 1237 mV while shutdown-manager
  logged `batteryLevel @ 3.720` in the same minute, so batteryLevel is exactly
  3.0 x ADCIN0; the 6.6 reading of 835 counts is 3.67 V. One 29-min 6.6
  session cost about 70 mV (3.789 -> 3.720 V).

### Jump 53: the backplate protocol, decoded from a 2-minute sniff
- `bpsniff.sh` opened `/dev/ttyS2` on the live 6.6 (115200 raw) and read 3358
  bytes in 120 s without sending anything: the backplate streams on its own.
  Frame format (both directions, `bptool.c`):
  `d5 d5 aa 96 | type u16 LE | len u16 LE | payload[len] | crc16 LE`, where
  crc16 is CRC-16/XMODEM (poly 0x1021, init 0, no reflection) over
  type+len+payload. Brute-forced against 7 distinct captured frames, then
  confirmed live: `bptool sniff /dev/ttyS2 12` decoded 27 frames, 0 bad CRC.
  `nlclient` imports `crc16_ccitt` for the same job.
- Messages seen (backplate -> head unit): type 0x0002 len 4 = SHT20 temperature
  and humidity in centi-units (0x0943/0x01db = 23.71 C / 47.5 %RH, about once a
  minute); type 0x0007 len 2 = 0 every ~2.5 s; type 0x000a len 4 = two u16
  (ambient-light-like, changed from 0x3f/0x43 to 0x1e3/0x0e between two
  sessions) every ~2.5 s; type 0x000b len 16 once (power scan: 4046, 4917 ...).
- `bptool` (static musl, 86 KB, `tools/kernel-firstlight/bptool.c`) has
  `sniff`, `send <type> [payload]` and `beat <type> <payload|-> <interval>
  <seconds>` modes and is now packed into the initramfs as `/bin/bptool`.
  The head-unit -> backplate message types (the status requests stock sends
  about every 11 s, and whatever feeds the 29-min watchdog) are being
  recovered from `nlclient` with rizin (`SP/re/`); nothing is sent to the
  backplate until a type is positively identified, because the same channel
  carries the HVAC switch commands.
- The backplate firmware itself (decoded `SP/bpfw/amber.bin`, Thumb-2 at
  0x08000000) confirms the reset is a head-unit watchdog, not a coincidence.
  The main loop at `0x14a8` calls `0x1f08`, which reads a flag byte and then
  either power-cycles the head unit (`0x3c50`, "Disconnecting HU") or pulses
  its reset line (`0x3c14`, "reset the HU now", which first checks a
  head-unit-present GPIO and otherwise logs "No HU, skipping reset of the
  HU"). A free-running tick at `0x200022e4` versus a stored "last activity"
  word is what the ~28.7 min timeout counts; receiving a valid frame from the
  head unit refreshes it. That is why stock, sending a status frame every
  ~11 s, is never reset, and why every headless 6.6 session dies at ~29 min.
- Actionable conclusion: to run longer than ~28 min on 6.6 the kernel must
  send the backplate one valid frame periodically. The framing and CRC are
  solved. This particular unit is a loose development board with no HVAC
  wiring attached, so sending frames cannot switch a real circuit and a
  keepalive can be tried freely with `bptool beat` on `/dev/ttyS2`.
  Full notes: `SP/bp-protocol-notes.txt`.

### Jump 54: the display works, confirmed by eye
- With the user standing at the device, jump 54 ran the `nest_fl_bl` colour
  test and played the host tune 11 s after the kexec. The user saw **white,
  then red, then green, then blue** on the panel. The display stack on 6.6 is
  therefore fully working end to end: DPI controller, S6D05A1 panel and LM3530
  backlight all do their job, and the framebuffer colour fills are visible.
  This closes the "needs eyes on the device" question from jumps 49-51.
- The colours came up **with kernel console text drawn over them**. That is
  fbcon: the cmdline carries `console=tty0`, so the kernel log and the init's
  banner render on tty0 on top of the raw `/dev/fb0` fills. It confirms the
  console/fbcon path renders too, not just direct framebuffer writes. For a
  clean image later, drop `console=tty0` (or move the console to ttyGS0 /
  ttyS-only) and paint from a vt that fbcon is not scribbling on.

### Jumps 55-56: the backplate keepalive cannot work from the head-unit UART
- Jump 53's actionable conclusion ("send the backplate one valid frame
  periodically to refresh the ~28 min timeout") turned out to be **wrong**.
  Jumps 55-56 tested it and a re-read of the AMBER_BP firmware explains why.
- Jump 55 failed for an unrelated reason: 802.11 auth timed out (the AP was
  found but `auth_failures=4`, SSID temp-disabled), so with no WiFi there was
  no dropbear, no `/netup`, and the init warm-rebooted at the 150 s grace long
  before the 29 min watchdog mattered. The old keepalive was pushed over SSH,
  so no WiFi meant no keepalive at all.
- So the keepalive was moved **into the init**, independent of WiFi: a new
  cmdline flag `nest_fl_bpkeep` makes STAGE2 touch `/netup`+`/hold` (engaging
  the STAGE3 hold loop with no network, capped at `NEST_FL_HOLD_MAX`), send
  ASCII `g\r\n` on `/dev/ttyS2` every 10 s, and run `fbanim` on the panel.
  `fbanim.c` is now staged into the initramfs by `pack-initramfs.sh`.
- Jump 56 (`nest_fl_bpkeep`, hold cap 2400 s): the session **held the full
  ~29 min** (so the WiFi-independent hold works) and then took a **power-up
  cold reset at uptime ~1740 s** -- `twl_rtc: Power up reset detected.`,
  `Last boot caused by Reset`, ramoops `size field = 0` (DRAM wiped). That is
  the backplate HU-watchdog signature, completely unchanged by sending `g`.
- Firmware root cause (`SP/bpfw/amber.bin`, AMBER_BP v1.0.26): the reset lives
  in main loop `fcn.00014a8`. A counter at `0x20000620` increments once per
  tick; at `counter % 2160 == 0` it calls the reset executor `fcn.0001f08`
  (~29 min). The entire counter/reset block is skipped **iff the byte at
  `0x20000440` is non-zero**. The only writer of `0x20000440 = 1` is
  `fcn.000013e8` -- which writes it *unconditionally*, ignoring its argument --
  and that is reached *only* from the ASCII console `g` command
  (`fcn.00003da0` case 6 at `0x3ee2`). It is cleared to 0 only at main-loop
  startup (`0x1548`). **No binary-protocol / frame-RX path sets that flag or
  resets the counter**, so no frame the head unit can send disables the reset.
- The firmware drives **two USARTs** (USART1 `0x40013800`, USART2
  `0x40004400`; USART3 unused). The binary HU-link protocol (`d5 aa 96`,
  serializer `fcn.00000f58` -- which emits `0xd5` twice, the extra pad byte we
  saw) is on one; the ASCII console (line editor `fcn.00000d00` + parser
  `fcn.00003da0`) on the **other**. Jump 56 proves the console is not on the
  HU link: `g\r` would have set `0x20000440 = 1` had it reached the handler,
  yet the reset still fired. The console is a separate debug UART on the
  backplate PCB, not wired to the head unit.
- Ways to actually defeat the ~29 min reset, in increasing cost/risk:
  1. Wire a 3.3 V USB-serial to the backplate's console-USART test pads (the
     non-HU USART) and send `g` -> sets `0x20000440`, watchdog off. Hardware
     access, but clean and reversible.
  2. Reflash the STM32 backplate over its BSL with the production D2/D2D
     firmware v4.2.8 (`SP/bpfw/backplate-Production-TFE_BP_D2D-mono-v4.2.8.*`,
     BSL `...-BSL-bsl-v3.1.srec`). AMBER_BP looks like a demo/burn-in build
     ("Resetting HU due to Demo mode TO"); D2/D2D is what ships. Risky (can
     brick the backplate), needs BOOT0/BSL entry and a deliberate decision.
  3. Accept the ~29 min ceiling: keep sessions warm-rebooting under it
     (default caps 900/1500 s, ramoops preserved) and persist state across
     reboots. `nest_fl_bpkeep` + `fbanim` remain useful for long-run visuals.

### Keepalive frame identified: type 0xa1 (the fix for the ~29 min reset)
- The question "why does stock not reboot" cracked it. The ~29 min reset
  is NOT unavoidable: the backplate main loop (`fcn.00014a8`) has a countdown
  timer at `[sl+0x2c]`. The 2160-tick (~29 min) hard-reset counter only runs
  while that countdown is zero; a non-zero countdown puts the firmware in a
  decrement-and-reset-when-zero branch instead. The countdown is reloaded by a
  received frame of **type 0xa1** (handler at `0x1cfe`): it sets the countdown
  to `min((payload_u16 if len==2 else 0x1c20) + 0x708, 0xffff)` ticks.
- The dispatcher decodes cleanly: `0x1c22 r2 = uxth([r8])` (16-bit type),
  `subs r2,0x81; cmp r2,0x42; tbh [pc,r2,lsl 1]` at `0x1c46` (table at
  `0x1c4a`) — valid types `0x81..0xc3`. Table index `0x20` (type `0xa1`) holds
  entry `0x5a` → `0x1c4a + 2*0x5a = 0x1cfe`.
- Cross-referenced in nlclient (the head-unit side): type `0xa1` is
  *"backplate controller: sending quiet message (max_sleep_seconds: %u)"*
  (`SendMessage` `fcn.001c54d4`, type arg `0xa1`). The head unit sends it to
  say "I'll be quiet at most N seconds, don't reset me" — i.e. the keepalive.
  Stock sends it periodically; 6.6 never did, so its countdown stayed 0 and the
  2160 counter reset it at ~29 min.
- So the keepalive is: **beat a type-0xa1 frame on `/dev/ttyS2` periodically**,
  payload = `max_sleep_seconds` (u16 LE). The init's `nest_fl_bpkeep` block now
  runs `bptool beat /dev/ttyS2 a1 5802 15 999999` (max_sleep 600, every 15 s)
  instead of the (wrong) ASCII `g`. `bptool` builds `d5 aa 96 | type | len |
  payload | crc16` exactly as nlclient's serializer `fcn.001daf50` does.
- This unit runs AMBER_BP v1.0.26; no HVAC is wired, so beating frames is safe.
  To be confirmed by a >29 min 6.6 session (jump 57): survive past ~1740 s and
  warm-reboot with ramoops intact = keepalive works.

### Jump 57: the 0xa1 keepalive WORKS — the ~29 min ceiling is gone
- Jump 57 booted 6.6 with `nest_fl_bpkeep`, which beat a type-0xa1 frame on
  `/dev/ttyS2` every 15 s (`bptool beat ... a1 5802 15 999999`, bptool pid 401
  in the ramoops) and held the session with no WiFi (`/netup`+`/hold`, cap
  2400 s). It reached **peak uptime 2383 s (~40 min)** and then ended via the
  init's OWN STAGE5 warm reboot at HOLD_MAX — `Last boot caused by Watchdog`,
  ramoops valid (`size field` non-zero). No `Power up reset` / `Last boot
  caused by Reset` / wiped ramoops: the backplate did **not** cold-reset us.
- Contrast: jumps 52 and 56 (no keepalive, or the wrong ASCII `g`) died at
  ~1740 s with a backplate power-up cold reset and lost ramoops. So type 0xa1
  is confirmed as the keepalive and the backplate's ~29 min head-unit watchdog
  is defeated. Headless 6.6 can now run until the battery or the chosen cap.
- Practical: raise `NEST_FL_HOLD_MAX` for long sessions and watch the battery
  (~70 mV per 29 min). The animation (`fbanim`) is a nice live liveness display
  on the round panel for the duration.

### Yocto packaging + LM3530 backlight driver (2026-10-31, not yet jumped)
- The kernel sources moved into the Yocto layer: `meta-j49/recipes-kernel/linux/
  linux-j49_6.6.bb` (kernel.org 6.6.64 + `files/000*.patch` + `j49.cfg` + the
  DTS). `build.sh` reads the same files, so a first-light kernel and a Yocto
  kernel are identical. Every patch now carries an `Upstream-Status:` line
  (blacksail's QA makes a missing one fatal).
- New `files/0006-backlight-lm3530-bl-dt-driver-nest-j49.patch`: a small
  backlight-class driver (`drivers/video/backlight/lm3530_bl.c`, compatible
  `ti,lm3530`, `hwen-gpios` + `vcc-supply`) that replays the stock init
  sequence from the `nest_fl_bl` block above and exposes
  `/sys/class/backlight/lm3530` (max 120, default 108 = stock). The DTS has
  the node on `&i2c3` (0x36, HWEN gpio177 = `&gpio6 17`, VAUX1) and the panel
  node now has `backlight = <&backlight>` (patch 0004 calls
  `drm_panel_of_backlight`), so blanking the panel switches the backlight.
  Compile-tested only (patches apply to a pristine tree, W=1 clean, DTB ok);
  the next jump must confirm the panel lights up without the `nest_fl_bl`
  i2cset hack. If it does, drop that block from the init.
- Patch 0003 was rewritten the same day: instead of hard-coding `sys_clkout1`
  into `enable_init_clks[]` (drivers/clk/ti/clk-3xxx.c), `twl-core.c` now takes
  its `"fck"` clock with `devm_clk_get_enabled()` (it only read the rate before)
  and the DTS gives the twl node `clocks = <&sys_clkout1>; clock-names = "fck"`.
  Same effect — the PMIC's HFCLKIN survives `clk_disable_unused()` — but
  expressed in the DT and upstreamable. Side effect to watch on the next jump:
  with a clock present, `clocks_init()` now also programs `R_CFG_BOOT`
  (HFCLK_FREQ_19p2 | HIGH_PERF_SQ) like the stock 2.6.37 did; jumps 8-57 skipped
  that ("Skipping twl internal clock init"). If I2C1 misbehaves again, the old
  clk-3xxx variant is in git history (commit before this change).

### Jump 58 (2026-10-31 18:12): first Yocto-built kernel, backlight driver, twl clock, bpd — all good
- Kernel = the `linux-j49` zImage + DTB from `build/tmp/deploy/images/nest-j49/`
  (Yocto gcc 16.2), turned into a kexec blob the usual way (`cat zImage dtb`,
  bump the u32 at 0x2c). Boots exactly like the first-light kernel: WiFi + SSH
  after ~2 min, 0 oopses, 43 MB MemTotal with `cma=0`.
- Patch 0003 v2 (twl-core keeps `"fck"`): `clk_summary` shows `sys_clkout1`
  enable_cnt 1 with consumer `0-0048 fck`; 0 TWL/I2C errors over the run, and
  `clocks_init()` programming `R_CFG_BOOT` had no visible side effect.
- Patch 0006 (LM3530 backlight): `/sys/class/backlight/lm3530` comes up at
  108/120 with HWEN (gpio177 = sysfs gpio-689) high and VAUX1 on. Verified
  against the chip: `brightness` writes land in BRT_CTRL (readback carries
  bit 7: 0x6c reads 0xec), brightness 0 drops GEN_CONFIG from 0x1d to 0x1c,
  and `echo 1 > /sys/class/graphics/fb0/blank` switches the backlight off with
  the panel (`bl_power` 4, GEN 0x1c); unblank re-inits the panel and restores
  108. The `nest_fl_bl` i2cset block in the init is therefore obsolete.
- `bpd keepalive` (Rust, `tools/bpd`) ran instead of bptool. New protocol
  detail: the backplate answers *every* 0xa1 with a bundle — type 0x14 len 2
  `[00 00]` (ack) + SHT20 type 0x02 + type 0x0a — so the keepalive doubles as
  a sensor poll (95 of each in 1440 s).
- TWL RTC read 2000-01-01 (lost at an earlier cold reset; stock re-syncs it).

### Jump 59 (2026-10-31 22:17): display confirmed by eye on the Yocto kernel
- Same blob and cmdline as jump 58; the user was at the device. Kexec at
  22:17:15, tune at 22:17:26, and the panel lit with the first-light animation
  under the LM3530 DT driver (108/120) — no `nest_fl_bl` i2cset poke involved.
  That closes the last visual check: the whole display path (S6D05A1 DRM panel
  + `drm_panel_of_backlight` + lm3530_bl) works from DT alone.
- **Ring -> backlight test (same session, user at the device).** The ADBS-A330 optical
  finger-navigation sensor at I2C2 0x57 (i2c-1 on 6.6) answers out of the box
  (Product_ID 0x88 = the "A350" path in stock `drivers/input/misc/adbs-a330.c`), but
  until it gets stock's init sequence its Motion register reads 0xf9 (MOT|OVF) with
  Delta_Y stuck at +/-127. Init that works (all `i2cset -y 1 0x57`): `0x60=0xe4`,
  `0x61=0xc9`, `0x6c=0x00`, `0x1d=0x0e`, `0x62 = (0x62 & ~0xf0) | 0x10`, `0x60=0x84`,
  `0x63 = (0x63 | 2) & ~1`. Afterwards Motion idles at 0x79 and deltas read 0.
  j49 platform data: mode DELTA_Y, direction POS; on this unit clockwise gave
  *negative* delta_y, so the test loop (`ringbl.sh`, now packed into the initramfs)
  runs with direction -1 so clockwise = brighter. A ring turn is thousands of counts
  (deltas saturate at 127 per read; a shell loop of `i2cget` forks only manages a
  handful of reads per second), so 40 counts per brightness step gives roughly half
  a turn for the full 1..120 range. Reading Delta_X then Delta_Y until both are 0
  clears the motion flag, exactly as the stock work handler does. Real driver work:
  use the MOTION# interrupt (gpio108) and report REL_X like stock, which is what
  the UI layer will want anyway.
- **The CPU runs at 300 MHz.** BogoMIPS 299.8, `dpll1_ck` 300 MHz, no
  `/sys/devices/system/cpu/cpu0/cpufreq`: stock's ondemand governor parks the MPU
  at OPP50 when idle and `kexec` hands that over. omap2plus_defconfig builds
  `CPUFREQ_DT=m`, so with a module-less kernel the `cpufreq-dt` platform device sits
  there without a driver even though `ti-cpufreq`, the omap36xx OPP table and
  `cpu0-supply = <&vcc>` (twl VDD1, already at 1.2 V = OPP100) are all in place.
  `j49.cfg` now has `CPUFREQ_DT=y` + `CPUFREQ_DT_PLATDEV=y` (+ `CPU_FREQ_STAT`);
  jump 60 verifies. This also explains why `fbanim` crawls at ~0.5 fps: it does
  double-precision `sqrt`/`atan2`/`fmod` + HSV per pixel (102 400 pixels/frame) on a
  VFPlite at 300 MHz, and the fbdev `write()` path moves 11.5 MB/s (34 ms per
  400 KB frame). The SGX530 GPU in the OMAP3630 is no help: no usable open driver,
  stock did not use it either; a UI should render with the CPU (NEON, integer math)
  and use the DSS overlays for scaling/blending.
- **Ring click = PWRON on the TPS65921.** Not an OMAP GPIO (a sysfs export of all 192
  lines + `/sys/kernel/debug/gpio` polling showed nothing on presses; the only input
  that ever moved was gpio108 MOTION# when the ring was touched) and not a TWL GPIO
  or keypad line either. `STS_HW_CONDITIONS` (0x4b reg 0x45) bit 0 `STS_PWON` goes
  0x70 -> 0x71 for exactly the duration of each press. Mainline: `twl4030-pwrbutton`
  (node already in `twl4030.dtsi`, interrupt 8 of the twl irq chip) reports
  `KEY_POWER`; `j49.cfg` now builds it and evdev in. Caveat for the Yocto image:
  systemd-logind powers the system off on KEY_POWER by default, so the image needs
  `HandlePowerKey=ignore` (logind.conf drop-in) before this driver goes into a
  rootfs with systemd.

### Jump 60 (2026-10-31 23:01): cpufreq-dt built in -> black screen, no WiFi, no reset
- Same blob as 59 plus `CPUFREQ_DT=y`, `CPUFREQ_DT_PLATDEV=y`, `INPUT_EVDEV=y`,
  `INPUT_TWL4030_PWRBUTTON=y`, default governor ondemand. The panel never lit (black
  from the start, per the user), the device never pinged, and nothing reset it: the
  HW watchdog is only armed by `/init`, so a hang before that point sits there until
  the backplate's ~29 min cold reset (or a long ring press on PWRON). Prime suspect:
  the first ondemand transition 300 -> 600 MHz (dpll1 relock + VDD1 step via the twl
  regulator) right after cpufreq init, which lands before the deferred DRM probe.
  Jump 61 = same kernel with `cpufreq.default_governor=userspace` on the cmdline, so
  the MPU stays at 300 MHz until the switch is made by hand over SSH with the
  watchdog armed and the DRAM ring live.
- Postmortem of 60 was unreadable: the recovery was a 10 s ring press (TPS65921 PWRON
  long-press -> PMIC reset, "Last boot caused by Reset"), which cycles DRAM, so the ring
  and ramoops zones came back as noise. Same for 61. Any early hang therefore needs
  bisecting by cmdline, not by dump.

### Jumps 61-62 (2026-10-31 23:19 / 23:29): bisect by `initcall_blacklist=`
- 61: same kernel + `cpufreq.default_governor=userspace` (no frequency transition at
  boot). Still black, no backlight, no ping -> the 300->600 MHz switch is NOT the
  trigger; the hang is in a probe path.
- 62: `initcall_blacklist=ti_cpufreq_init,cpufreq_dt_platdev_init,dt_cpufreq_platdrv_init`
  (pwrbutton kept). Boots, display + WiFi fine, `twl4030_pwrbutton` registers as
  `/dev/input/event0`. Verdict: the cpufreq stack hangs this board at probe time;
  the power-button driver is innocent. 63 blacklists only `dt_cpufreq_platdrv_init`
  to separate ti-cpufreq's probe (efuse read at CONTROL_DEVICE_STATUS 0x4800244C /
  IDCODE 0x4830A204 through syscon or ioremap) from cpufreq-dt's policy init.
- Button events: the TWL top-level interrupt (`TWL4030-PIH`, INTC 7) has counted 0
  in every 6.6 session so far, so KEY_POWER events depend on SYS_NIRQ actually
  reaching the INTC; pad 0x480021e0 reads 0x4118 (mode 0, input, pull-up, wakeup),
  PWR_IMR1 = 0xf2 (PWRON unmasked). Polling `STS_HW_CONDITIONS` bit 0 works
  regardless. Being measured during jump 62.
- 63 (2026-11-01 00:12): `initcall_blacklist=dt_cpufreq_platdrv_init` only. Boots;
  `ti-cpufreq` probed (driver bound, `cpufreq-dt` platform device created, no
  errors) -> the efuse/IDCODE reads are fine and the hang is inside cpufreq-dt's
  probe / `cpufreq_online` (OPP table add, regulators cpu0+vbb, clk, first policy
  init). Next: build cpufreq-dt as a module, insmod it over SSH with the HW watchdog
  armed and the DRAM ring live, so the hang (or oops) leaves a readable trace.
- 64 (00:20): cpufreq-dt rebuilt as a MODULE (`CPUFREQ_DT=m`, `cpufreq-dt.ko` in the
  initramfs, nothing loaded), no blacklist. Black screen, no ping: hangs even though
  the active code equals jump 63, which booted. So this is not "the cpufreq-dt probe";
  it is intermittent, or it depends on something the blacklist changed as a side
  effect. Lesson: without a trace every further bisect is a coin flip. 65 therefore
  boots the same blob with `omap_wdt.early_enable=1 omap_wdt.timer_margin=60`
  (WDT2 started at the driver's own probe, kicked by the watchdog core until /init
  opens it) plus `initcall_debug`, so a hang after that probe becomes a 60 s warm
  reset with the DRAM ring intact and the last initcall named in it.
- 65 (00:43): same blob as 64 + `omap_wdt.early_enable=1 omap_wdt.timer_margin=60
  initcall_debug`. BOOTED (so 64's hang was not deterministic for this blob), /init
  armed the watchdog at 4.9 s. Then, over SSH with the WDT armed: `insmod
  /cpufreq-dt.ko` returned 0 and the board was dead within the next ~3 s (no ping,
  SSH stalled). That is the reproducer: the hang sits in cpufreq-dt's
  `cpufreq_online` path (OPP/regulator setup, policy init, first ondemand transition),
  not in ti-cpufreq. The WDT reset that follows keeps DRAM, so decode65 has the ring.
  Side notes: no `/sys/class/watchdog` (WATCHDOG_SYSFS off, /dev/watchdog works);
  dmesg had wrapped by 9 s with `initcall_debug` on (LOG_BUF_SHIFT now 18 in j49.cfg).

### Jump 66 (2026-11-01 01:04): the DPLL is fine, 600 MHz is fine -- it is the 800 MHz OPP
- `cpufreq.default_governor=userspace` + `omap_wdt.early_enable=1`, module kernel.
  `dplltest.sh` (scratchpad) under the armed watchdog: registers at T0 were
  CM_CLKEN_PLL_MPU 0x37 (locked), CM_CLKSEL1_PLL_MPU 0x00107D07 (M=125 N=7,
  MPU_CLK_SRC=2 -> dpll1_fck 166 MHz bypass), M2=1, ABB_CTRL 0x4B01. Clock tree:
  dpll1 300 / x2m2 600 / mpu_ck 600 / arm_fck 300 MHz (BogoMIPS 299.8, so the ARM
  runs at arm_fck = dpll1 rate).
- Manual relock by devmem (LP bypass -> M 125->250 -> lock) worked: shell loop time
  dropped 31 s -> 12 s, 2 s later still alive, relock back fine.
- Then `insmod cpufreq-dt.ko`: policy came up as userspace at 300000, available
  300000 600000 800000. `scaling_setspeed 600000` worked (kernel chose N=3, M=125,
  CLKSEL1 0x00107D03, dpll1 600 / mpu_ck 1200 / arm_fck 600 MHz), back to 300000 fine.
- So: DPLL1 relock OK, 600 MHz at the inherited VDD1 1.2 V OK. The killer is what
  ondemand does at once under boot load: jump to the max OPP, 800 MHz (OPP130,
  VDD1 1.325 V + ABB). Follow-up in the same session: read VDD1/ABB, setspeed 800000
  -> SSH stalled immediately, no ping (verdict/markers from the ring below).
- The 800 MHz attempt (01:10): ring markers show VDD1 1012 mV / VSEL 0x21 at 300 MHz
  and 1200 mV / VSEL 0x30 at 600 MHz (so the twl regulator path works), then
  "setspeed 800000" and silence -> WDT reset. OPP130 is the killer; the DTS now
  disables `opp-130-800000000`.

### Jump 67 (01:28): OPP130 disabled, cpufreq-dt built in, early WDT -- early hang again
- No SSH, no ping, and the early-enabled watchdog did NOT reset it: recovered only by
  the backplate's cold reset ~29 min later ("Last boot caused by Reset", DRAM lost).
  So this is the 64-type hang: before the watchdog driver probes (or before the
  WDT is effective), unrelated to the OPP. DTB verified: the 800 MHz node carries
  status = "disabled". Tally with ti-cpufreq + pwrbutton active: 63, 65, 66 booted;
  64, 67 hung. 68 runs the same kernel with `initcall_blacklist=
  twl4030_pwrbutton_driver_init`.
- 68 (02:01): same kernel, `initcall_blacklist=twl4030_pwrbutton_driver_init`. No SSH,
  no WDT reset -> the power-button driver is not it. Tally for built-in cpufreq-dt +
  ondemand at boot: 60, 61, 67, 68 all hang; the only built-in configs that booted
  (62, 63) had cpufreq-dt blacklisted. Theory that fits everything incl. jump 66:
  `twl-regulator` defines no ramp_delay, so the OPP core switches DPLL1 to 600 MHz
  right after the VDD1 VSEL I2C write while the rail is still slewing up from
  1.0125 V; ondemand's rapid 300<->600 flips under boot load race it, my slow manual
  steps in 66 did not. (64 - module blob, nothing loaded - stays unexplained.)
  Jump 69: OPP50 pinned at 1.2 V in the DTS (no voltage ramps at all; stock idles
  at 300 MHz / 1.2 V too), cpufreq-dt built in, ondemand.
- 69 (02:35): OPP50 pinned at 1.2 V, cpufreq-dt built in, ondemand, early WDT. No SSH,
  no WDT reset. The voltage-ramp theory is dead too.

### Where the cpufreq hang stands (2026-11-01 02:50)
- Hung: 60, 61, 64, 67, 68, 69. Booted: 62, 63, 65, 66 (and every jump up to 59).
  The only consistent split: with cpufreq-dt built in and nothing blacklisted the boot
  hangs 5/5; with its initcall blacklisted it boots 2/2; as an unloaded module 2/3.
  64 (module, nothing loaded) is the one that does not fit. Not the OPP130 entry, not
  the OPP50 voltage ramp, not the power-button driver, not the 300->600 transition
  itself (works by hand). No trace exists because the hang precedes the point where
  the omap_wdt probe (even with early_enable) or /init can arm the watchdog, and
  every recovery (PMIC long-press, backplate cold reset) wipes DRAM.
- Next step that actually buys evidence: arm WDT2 from *stock* right before `kexec -e`
  (unbind stock's omap_wdt so its shutdown hook cannot stop it, then start WDT2 by
  devmem with a ~60 s reload). 6.6 has WATCHDOG_HANDLE_BOOT_ENABLED, so a healthy boot
  keeps kicking it; an early hang becomes a warm reset with the DEBUG_LL ring intact.
- Until then j49.cfg keeps `CPUFREQ_DT=m` (safe) and the DTS keeps OPP130 disabled and
  OPP50 at 1.2 V (both harmless, both matching stock behaviour).
- 69 postmortem, read properly: NOT a hang. ramoops valid, the session ran its full 2408 s
  (bpd kept the backplate happy) and ended with the normal STAGE5 warm reboot. WiFi was
  the failure: scan fine (-51 dBm), but every auth frame went unacknowledged (3 tries
  10 ms apart, "authentication timed out"), i.e. TX broken from the first attempt at
  14.7 s. 67/68 were cold-reset by the backplate at ~29 min, so there bpd was not
  running -> dead or TX broken too.
- 70 (03:20, boot config of 66): module pushed over SSH and loaded live at 515 s;
  userspace 600 MHz for 60 s, then ondemand capped at 600 MHz for 120 s = 623
  transitions: host pinged the device 72/72, SSH stayed up, bpd acks kept coming.
  So at runtime neither 600 MHz nor ondemand's switching breaks WiFi/UART TX or the
  CPU. The failures are specific to cpufreq being active DURING boot (WL1271 firmware
  boot at ~8 s, auth at ~14 s). 71 reproduces that under watchdog cover: the init
  gains `nest_fl_insmod=` (insmod right after arming the WDT, ~5 s), module kernel
  with OPP130 disabled, ondemand.
- 71 (04:02): never jumped. chain47 pinned stock, but all 40 push attempts over 20 min
  came back "not our device / unreachable" (stock dropped off WiFi right after the
  pin). Nothing ran on the device; rerun when convenient:
  `NEST_ZIMAGE=$SP0/kbuild/zImage-dtb-kx-cpufreqmod2 NEST_CL="... nest_fl_bpkeep
  nest_fl_insmod=/cpufreq-dt.ko omap_wdt.early_enable=1 omap_wdt.timer_margin=60 ..."
  bash chain47.sh 71`.

### Phase 3: the Yocto image itself from RAM (jumps 72+, 2026-11-01)
Goal: run the real `j49-image` (systemd PID 1, networkd + wpa_supplicant, dropbear, bpd)
on the hardware before anything touches NAND. Build 12 adds `squashfs-xz` to the image
(14.4 MB for the 43 MB rootfs) and `CONFIG_SQUASHFS(_XZ)` + `CONFIG_OVERLAY_FS` to the
kernel. The kexec initrd is `pack-squash-initramfs.sh`: static busybox + `squash-init` +
the squashfs + a provisioning overlay (secrets, kept in the scratchpad: wpa conf, dropbear
host keys, authorized_keys, device NVS; plus `nest-session.{sh,service}`, a guard that logs
health lines to kmsg every 2 min and warm-reboots to stock after 2400 s). `squash-init`
loop-mounts the squashfs, puts a 24 MiB tmpfs overlay on it, copies the overlay tree in
and `switch_root`s to `/sbin/init`. New `j49-config` recipe in the image: `80-wlan0.network`
(DHCP, ClientIdentifier=mac), `RuntimeWatchdogSec=30s`, `wpa_supplicant@wlan0` enabled.
- 72 (12:19): never reached /init. `Memory: 18500K/63488K available (13312K kernel code
  ... 36796K reserved, 8192K cma-reserved)`: with the 15 MB initrd still reserved, only
  18.5 MB (+8 MB CMA) is managed when the initramfs is unpacked, and the rootfs tmpfs is
  capped at half the managed pages (~13 MB) -> `rootfs image is not initramfs (write error)`
  on the 14.4 MB squashfs, fallback `/initrd.image: incomplete write (-28)`, then
  `VFS: Unable to mount root fs on unknown-block(0,0)` at 4.2 s, panic=15 -> stock. Fix:
  `rootfstype=ramfs` (no cap). Note for later: 13 MB of kernel code is omap2plus_defconfig
  with everything built in; trimming it is the big memory lever on 64 MB.
- 73 (12:26, + `rootfstype=ramfs`): NO LOG. Stock came back at ~12:55 with "Last boot caused
  by Reset" = the backplate's ~29 min COLD reset (bpd was not feeding it), DRAM wiped,
  ramoops zone empty. The 60 s omap_wdt never fired, so the kernel stayed alive and the
  WDT was pet (by the kernel worker if userspace never opened it, or by systemd). So:
  either squash-init/systemd stalled with the kernel idle, or systemd ran but bpd.service
  and WiFi both failed -- and nest-session's 2400 s hold was LONGER than the 29 min
  backplate limit, so it never got to warm-reboot. Fixes for 74: hold 1500 s; a chroot'ed
  deadman spawned by squash-init that warm-reboots at 600 s unless nest-session.service
  started (`/run/nest-session.started`); `printk.devkmsg=on systemd.log_target=kmsg
  systemd.log_level=info` so systemd's own log lands in the ramoops zone; journald
  volatile + 2 MB.
- 74 (13:09, + deadman/kmsg logging, NVS + regdb in the initramfs root): HARD EVIDENCE.
  `Kernel panic - not syncing: System is deadlocked on memory` at 1.36 s, inside the
  initramfs unpack: `unevictable:11364kB` (the ramfs copy so far), `free:8584kB` of which
  `free_cma:7936kB` (unusable for the kmalloc of an i2c probe), 9200 pages reserved. The
  arithmetic: 64 MB - 20 MB kernel (13.3 MB text, omap2plus_defconfig all built in) - 15 MB
  initrd - 8 MB CMA = 18.5 MB managed, and the unpack needs the whole payload a second time.
  A squashfs inside the initrd can never fit; the earlier -ENOSPC was just the first symptom.
  Also seen on 72: `Direct firmware load for ti-connectivity/wl127x-nvs.bin failed` at
  3.7 s -- wl12xx wants the NVS BEFORE switch_root, so it must be in the initramfs root
  (jump 73's silence = no wlan0 at all + bpd absent -> backplate cold reset at 29 min).
- Netboot path (75+): `netboot-init` + `pack-netboot-initramfs.sh`, a 1.8 MB initramfs
  (busybox, static wpa_supplicant/dropbear, secrets, NVS/regdb). It associates, DHCPs,
  starts dropbear; the host (`netpush.sh N`) pushes the 14.4 MB squashfs over SSH, verifies
  the md5 and touches /go; the init releases the radio and execs `squash-init`
  (loop + tmpfs overlay + provisioning + chroot'ed deadman + switch_root to systemd).
  Memory: the initrd is freed before the payload arrives, so the peak is one copy.
- 75 (13:44, netboot): the loop works -- `NEST-NB: start ... MemTotal: 42688 kB MemFree:
  24048 kB` at 3.9 s (so 42.7 MB managed once the small initrd is freed), then
  `FAIL: wlan0 up: wlcore: ERROR could not get firmware ti-connectivity/wl127x-fw-5-sr.bin`
  at 5.9 s: wlcore loads the main firmware on `link up`, i.e. in this initramfs, not only
  the NVS. The init's fail path warm-rebooted and stock was back 6 min after the jump with
  ramoops intact. Fix: pack-netboot-initramfs.sh takes a firmware tree (5th arg).
- 76 (13:53, + WL1271 firmware in the initramfs): WiFi associated 2 s after `up`, DHCP
  lease 192.168.1.50, dropbear up at 12.6 s, host started pushing at 171 s. Then the OOM
  killer ate wpa_supplicant, the backstop `sleep` and finally init while the zone still had
  `free:16512kB` -- because `boost:8192kB` had lifted `min` to 8984 kB (watermark boosting
  on fragmentation) and the ramfs payload pages are unmovable (no CMA for them). Fixes for
  77: `vm.watermark_boost_factor=0` (init + `/etc/sysctl.d/10-j49-mem.conf` in j49-config),
  payload in a 20 MiB tmpfs (movable -> CMA-eligible), tcp_rmem/wmem capped at 1 MB.
- 77 (14:03, tmpfs payload + watermark boost off): THE IMAGE BOOTED INTO SYSTEMD. Push OK
  (md5 verified at 181 s, MemFree 7.0 MB), squash-init: squashfs + overlay mounted,
  provisioning copied (cp returned 1 -- to check), switch_root at 183.5 s with
  `MemFree: 6692 kB`. systemd 261.3 came up as PID 1, set the hostname, took
  `/dev/watchdog0` (30 s), queued multi-user.target and worked through sockets/slices/mounts
  until the log stops at 212 s ("Load Kernel Module drm skipped"); stock was back at
  14:08 with "Last boot caused by Watchdog". Reading: no OOM messages, so systemd + udev +
  journald thrashed on the ~6 MB that was left next to the pinned 14.4 MB squashfs and the
  20 MB kernel, until PID 1 missed the watchdog. Memory is now the only blocker -> kernel
  size trim in j49.cfg (other SoCs, SMP, HIGHMEM, NFS, ext4, PCI/SCSI/ATA, netfilter,
  kprobes/perf/ftrace, io_uring, ...), kexec-tools/kdump out of the image, zram available.
- Build 13 (14:35): trimmed kernel = zImage 4.32 MB (was 6.26), vmlinux text 9.5 MB (was
  ~16.4 MB code+rodata), data 3.0 MB; all board drivers still =y (checked WL12XX, MMC_OMAP_HS,
  TWL4030 core/MADC/RTC/pwrbutton/regulator, OMAP_WATCHDOG, DRM_OMAP + S6D05A1 panel,
  LM3530_BL, MUSB + g_serial, NAND/JFFS2/UBIFS, squashfs/overlay, PSTORE_RAM, cpufreq-dt=m).
  Image without kexec-tools: squashfs 14.0 MB, jffs2 21.7 MB, 81 packages.
- 78 (14:35, trimmed kernel): `Memory: 38108K/64512K available (9216K kernel code ...
  18212K reserved)`, init at 1.8 s with `MemTotal: 49440 kB MemFree: 31580 kB` (was 42688 /
  22928). But no association in 90 s: every auth frame to both BSSIDs of the SSID
  (ch 1 and ch 11) "timed out" with the 3 tries 13 ms apart = the firmware reported
  no-ACK immediately (mac80211 retries at once on an unacked MLME frame). Same signature
  as 69; 76/77 also lost their first auth and succeeded on the retry. So WiFi TX has been
  marginal all along, independent of cpufreq and of the trim. Suspect: the NVS in use is the
  generic linux-firmware one (912 B, identical to wl1271-nvs.bin) = generic RF calibration.
  Correction after counting across the old ramoops logs: auth timeouts happen on EVERY
  boot (55: 8, 58: 3, 59: 5, 62: 7, 63: 3, 65: 5, 66: 1, 69: 15, 70: 2, 76: 1, 77: 2 before
  the first "authenticated"), including 69, which did associate in the end. So 78 was the
  same flakiness within a 90 s budget, and the "boot-time cpufreq breaks WiFi TX" reading of
  69 is NOT supported. The NVS in use is the device's own (system-config wl12xx-nvs.bin,
  md5 b6409d31), not the generic one. Why the first auth frames go unacked is a separate
  open item (RF/power-save/ROC timing); netboot-init now allows 3 x 100 s with a firmware
  restart in between.
- 79 (14:42, 3 x 100 s + firmware restarts): still no association -- 7, 17, 25 cumulative
  auth timeouts, `wpa_state=SCANNING` throughout, `CONN_FAILED`. Two boots in a row with
  the trimmed kernel and zero ACKed auth frames, while stock was pinned over WiFi minutes
  before each jump; the untrimmed kernel (76, 77) associated within 10 s both times. The
  trim touches TX. Bisecting with first-light variant kernels.
- 80 (15:03, kernel variant A = trim with the SoC/SMP/HIGHMEM block restored, built by
  kvariant.sh): associated in round 1 after a firmware restart (8 auth timeouts), push OK at
  154 s, switch_root at 157 s with `MemFree: 14636 kB` (variant A: 48.6 MB managed). systemd
  then ran for 4 minutes: journald, userdb, udevd, resolved, timesyncd, tmpfiles all came up
  -- but at a crawl (udevd "16.7s CPU over 2min 43s wall", timesyncd "13s CPU over 1min
  41s"), so the 90 s device-unit jobs for ttyS2/ttyGS0/wlan0 expired before udevd existed
  and bpd / wpa_supplicant@wlan0 / serial-getty failed on "dependency". Two causes: every
  kmsg line (systemd.log_target=kmsg + ignore_loglevel) was rendered on the fbcon panel
  (console=tty0), and page-cache thrash with xz squashfs faults on ~14 MB. Also seen:
  `cp: target '/newroot/./lib' is not a directory` (usrmerge symlink; overlay copy fixed
  per top-level entry). For 81: console=ttyGS0 (ramoops/ring keep everything), zram swap
  32 MiB in squash-init, DefaultDeviceTimeoutSec/DefaultTimeoutStartSec=300s
  (j49-config 20-j49-timeouts.conf), journald no longer forwards to kmsg.
- 81 (15:29, variant A + console off the panel + zram + 300 s timeouts): log LOST. The
  retried ramoops read (stock returned at 16:01, ~31 min after the jump = the backplate's
  ~29 min cold reset) showed a corrupted zone: ramoops sig garbage, DEBUG-LL ring with
  ~15000 bit errors in 30 KB (A/B copies disagree on half the bytes). Readable early lines
  confirm variant A booted (panel model string, kernel built 14:53) but nothing past that.
  The cold reset means bpd never fed the backplate within 29 min -- in 80 bpd.service was
  gated behind udev + dev-ttyS2.device, which crawl at 300 MHz, so it timed out. Decisions:
  (1) start bpd from squash-init (chroot into /newroot, image's own /usr/bin/bpd on the
  moved /dev/ttyS2), surviving switch_root, so the backplate is fed from ~3 min regardless
  of systemd -- the image's bpd.service becomes a restart owner only; (2) adopt variant A as
  the kernel trim (j49.cfg keeps the other TI SoCs + SMP + HIGHMEM; the ~1 MB they cost is
  not worth the WiFi-association risk seen in 78/79). Rebuild = build 14.
