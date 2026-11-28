# Nest J49 NAND flash layout

> **How to read this page (28 November 2026).** The table below is the layout of the stock
> firmware; our system uses the same partitions. The sections after it are the record of the
> bring-up in the order it happened (October and November 2026). Many of them describe a state
> that is overtaken: the stock A slot as a fallback, the B slot, kexec from stock, the plans.
> They are kept as history. The state now, seen on devices unless said otherwise:
>
> - There are three ECC layouts on this flash: that of the stock firmware (BCH4, bytes 36..63
>   of the OOB), that of our kernel (mainline `bch4`) and that of our U-Boot (BCH8). None of
>   them reads what another wrote. Our kernel cannot read partitions the stock firmware
>   wrote, and the reverse.
> - So each partition has one writer: `spl0`/`spl1` are written under the stock service
>   kernel (the stock x-loader reads them), `boot0` by our U-Boot over USB, `root0`, `root1`
>   and `scratch` by our kernel.
> - On a thermostat that still has its stock data, the installer fetches the WiFi calibration
>   from `system-config` and erases `scratch` while the stock service kernel runs, because
>   our kernel cannot do either afterwards ([install.md](install.md)).
> - After an install there is no stock slot to fall back to: our U-Boot is in both boot loader
>   partitions. The way back is the backup the installer made.
> - Which root slot starts: a record in `env1`, read by `nandio`, the first program our kernel
>   starts ([update.md](update.md); tested on a PC only). Without a record the kernel command
>   line decides.
> - The kernel in `boot0` can only be written by U-Boot over USB
>   ([update-from-linux.md](../tools/dfu-install/update-from-linux.md)).

Source: `/proc/mtd` on the live device (firmware 5.9.4-5-j49-0NXR), captured
2026-10-25. NAND = 256 MB, erase block 0x20000 (128 KiB).

| mtd | name          | offset (hex) | size (hex) | size    | role |
|-----|---------------|-------------|------------|---------|------|
| 0   | nand0         | 0x00000000  | 0x10000000 | 256 MB  | whole chip |
| 1   | ipl           | 0x00000000  | 0x00040000 | 256 KB  | initial program loader (x-loader/MLO) |
| 2   | spl0          | 0x00040000  | 0x00180000 | 1.5 MB  | u-boot (A) |
| 3   | spl1          | 0x001c0000  | 0x00180000 | 1.5 MB  | u-boot (B, == spl0) |
| 4   | env0          | 0x00340000  | 0x00060000 | 384 KB  | u-boot env (A) |
| 5   | env1          | 0x003a0000  | 0x00060000 | 384 KB  | u-boot env (B) |
| 6   | boot0         | 0x00400000  | 0x00800000 | 8 MB    | kernel (A, == boot1) |
| 7   | root0         | 0x00c00000  | 0x02e00000 | 46 MB   | rootfs slot A (JFFS2) — stock boots here |
| 8   | boot1         | 0x03a00000  | 0x00800000 | 8 MB    | kernel (B) |
| 9   | root1         | 0x04200000  | 0x02e00000 | 46 MB   | rootfs slot B (JFFS2) |
| 10  | system-config | 0x07000000  | 0x00400000 | 4 MB    | JFFS2 |
| 11  | user-config   | 0x07400000  | 0x00800000 | 8 MB    | JFFS2 |
| 12  | data          | 0x07c00000  | 0x01400000 | 20 MB   | JFFS2 |
| 13  | log           | 0x09000000  | 0x01400000 | 20 MB   | JFFS2 (live) |
| 14  | scratch       | 0x0a400000  | 0x05c00000 | 92 MB   | JFFS2 temp |

Stock kernel cmdline: `root=/dev/mtdblock7 rootfstype=jffs2 console= nlmodel=Display-2.14 brightness=108 panic=1`

A/B design: spl0==spl1 and boot0==boot1 (redundant); root0/root1 are independent
rootfs slots. A modern image can install into the B slot (boot1/root1) first so
the stock A slot stays intact as a fallback during bring-up.

Make a full backup of every partition before you write anything, and keep it outside git.

## NAND chip, ECC format (from the stock kernel source, 2026-10-31)

- Chip: **Micron MT29F2G16ABBEAH4** (2 Gbit, x16, 2 KiB pages, 64 B OOB,
  128 KiB blocks) on GPMC CS0 (`board-diamond.c`; dev boards also list
  MT29F2G16ABDHC-ET and a Samsung K9F4G08U0C).
- Stock driver: `omap2.c` with `ecc_opt = OMAP_ECC_BCH4_CODE_HW`, no DMA,
  prefetch IRQ. **ECC = raw GPMC BCH4 syndromes**: 7 bytes per 512 B sector
  read straight from `GPMC_ECC_BCH_RESULT_x` (no constant polynomial added),
  28 bytes per page, stored at the **end of the OOB (bytes 36..63)**;
  bytes 2..35 are oobfree (JFFS2 cleanmarkers live there).
- Mainline 6.6 `ti,nand-ecc-opt = "bch4"` on OMAP3 (no ELM) means
  `OMAP_ECC_BCH4_CODE_HW_DETECTION_SW`: same 28 bytes but placed at OOB 2..29
  and with the erased-page polynomial folded in. **Incompatible** with the
  stock format in both position and value: it can neither read stock
  partitions cleanly nor write `boot1`/`root1` in a form the stock U-Boot /
  x-loader can read. Writing the B slot through the stock kernel (`nandwrite`
  from stock, which uses the stock ECC) sidesteps this for the rootfs; a
  modern kernel that mounts NAND itself needs a compat ECC variant in
  `drivers/mtd/nand/raw/omap2.c` (raw syndrome, layout at end of OOB).
- The DT node with the partition table is in `omap3-nest-j49.dts`
  (`&gpmc`, `status = "disabled"` until that is done).

## NAND B-slot bring-up strategy (decided 2026-11-01, from the stock u-boot env)

The stock env (`mtd4_env0.bin`) fixes the boot chain and the A/B failover:

```
bootcmd    = run nandboot0 || run nandboot1 || run mmcboot || reset
altbootcmd = run nandboot1 || run nandboot0 || run mmcboot || reset
nandboot0  = nboot.i ${loadaddr} 0 0x400000  && setenv bootargs root=/dev/mtdblock7 && run addjffs2 addtty addmodel && bootm ${loadaddr}
nandboot1  = nboot.i ${loadaddr} 0 0x3A00000 && setenv bootargs root=/dev/mtdblock9 && run addjffs2 addtty addmodel && bootm ${loadaddr}
loadaddr   = 0x81000000   bootfile = uImage   bootdelay = 0
```

So `nboot` loads a **uImage** kernel from boot0/boot1 using the x-loader/u-boot
NAND read (stock BCH4 ECC), `bootm` boots it, root is mtd7 (A) or mtd9 (B),
rootfstype jffs2, and `nlmodel`/`console=ttyO0`/`panic=1` are appended.

**Chosen split (no compat-ECC driver needed for bring-up):**
- **boot1 (mtd8)** = modern kernel as a **uImage** (ARCH arm, load/entry
  0x81000000, the appended-DTB zImage as data). Written from the **stock**
  kernel's `nandwrite` so it is in stock BCH4 and `nboot` can read it. The
  modern kernel never reads boot1.
- **root1 (mtd9)** = modern rootfs jffs2, **owned by the modern kernel** with
  mainline `bch4` (DTS `ti,nand-ecc-opt="bch4"`), written and read
  self-consistently. Stock u-boot never touches root1.

Because neither side reads the other's ECC, the stock-format compat variant in
`omap2.c` (OOB 36..63, raw syndromes) is **deferred** — it is only needed if a
modern u-boot must read a stock-written kernel, or if we flash root1 from stock.

**A slot stays the fallback:** ipl/spl0/spl1/boot0/root0 are `read-only` in the
DTS partition map, so the modern kernel cannot write them; `altbootcmd` already
falls back B→A. The DTS NAND node has **no `nand-on-flash-bbt`**, so a mainline
probe only *scans* OOB for bad blocks (read-only) — it does not write a BBT.

**Staged sequence (each device step in one held kexec session):**
1. READ-ONLY probe (safe): boot the modern kernel with the NAND node enabled,
   `cat /proc/mtd`, `dmesg | grep -i nand`, `nanddump --oob` a page of root1 and
   of an erased region. Confirms geometry (2 KiB/64 OOB), the bad-block scan and
   that MTD comes up. (root1 still holds stock data → ECC errors expected; fine.)
2. WRITE root1 from the modern kernel: `flash_erase /dev/mtd9 0 0`, then write the
   jffs2 image (`nandwrite`/`mkfs.jffs2`; needs mtd-utils in the initramfs/image),
   mount `-t jffs2 mtdblock9 /mnt`, verify.
3. WRITE boot1 from STOCK: build the uImage, push it, `flash_erase`+`nandwrite`
   from stock into mtd8 (stock ECC).
4. Point u-boot at B: set `bootcmd` to try nandboot1 first (keep nandboot0 as the
   `||` fallback), via stock `fw_setenv`. CONFIRM WITH USER before this write.
5. Cold boot: stock u-boot `bootm`s the modern kernel from boot1, which mounts
   root1. A-slot untouched; `altbootcmd`/the `|| nandboot0` tail recover stock.

Tools: busybox has `nanddump`/`nandwrite` only; `flash_erase` + `mkfs.jffs2`
come from mtd-utils (added to the image; cross-built static for the initramfs).

### Read-only probe on the device (2026-11-01, jump 83) — strategy validated

The modern kernel (build 14 + NAND node enabled) probed NAND cleanly and
NON-destructively:
- `nand: Micron MT29F2G16ABBEAH4 ... 256 MiB, SLC, erase 128 KiB, page 2048,
  OOB 64`, `using OMAP_ECC_BCH4_CODE_HW_DETECTION_SW`, 14 partitions created,
  no BBT written. sysfs: `ecc_strength=4 ecc_step_size=512 oobsize=64`.
- **Partition renumbering (important):** the modern kernel counts from mtd0=ipl,
  so **boot1 = mtd7, root1 = mtd8** (stock /proc/mtd and the u-boot env call them
  mtd8/mtd9). Stock `nandboot1` passes `root=/dev/mtdblock9`, which under the
  modern kernel is *system-config*. The modern kernel must therefore FORCE its
  own root: `CONFIG_CMDLINE="... root=/dev/mtdblock8 rootfstype=jffs2"` +
  `CONFIG_CMDLINE_FORCE` (or `EXTEND`), so u-boot's bootargs cannot misroot it.
- **Stock ECC layout confirmed on silicon:** a raw (`nanddump -n -o`) read of a
  root0 data page shows OOB bytes 0..35 = 0xff and 28 non-0xff bytes at
  **OOB 36..63** = the stock BCH4 ECC. Mainline bch4 places its 28 bytes at
  OOB 2..29 instead — incompatible, as expected. Our split (root1 written+read
  by the modern kernel's mainline bch4; boot1 written by stock, read by stock
  u-boot) never crosses the two layouts, so the compat driver stays deferred.

Write phase needs mtd-utils (flash_erase, mkfs.jffs2) — not in busybox; added to
the image and to be cross-built static for the write-capable initramfs.

### B-slot flashed (2026-11-01, jumps 84 + stock session)

Both B-slot partitions written and verified, stock A untouched:
- **root1** from the modern kernel (session jump 84): `flash_erase /dev/mtd8` then
  `/usr/sbin/nandwrite -p /dev/mtd8 -` streaming the build-15 jffs2 (mainline
  bch4). Mounts: os-release = Poky blacksail, /sbin/init -> systemd,
  /usr/bin/bpd present, 22.8 MB used of 46.
- **boot1** from stock: `flash_eraseall /dev/mtd8` then `nandwrite -p` the
  uImage. Header verified: magic 27051956, name "nest-6.6.64-j49",
  load/entry 0x81000000, size 0x484b88.

GOTCHAS (cost two redo rounds):
- The static mtd-utils live in /usr/sbin but the busybox/stock shell PATH is only
  /bin (initramfs) or similar, so `flash_erase`/`nandwrite` must be called by FULL
  PATH, else busybox's `nandwrite` (no erase) runs and the write lands on
  un-erased NAND (header/name corruption).
- Stock's old mtd-utils `flash_erase /dev/mtdX 0 0` erases 0 blocks ("Erase Total
  0 Units"); use `flash_eraseall /dev/mtdX` (or an explicit block count).

Remaining: point u-boot at B (bootcmd nandboot1 first), cold boot, confirm the
modern kernel loads from boot1 and mounts root1 (root=/dev/mtdblock8 forced via
CONFIG_CMDLINE_EXTEND).

### B-slot cold boot FAILED to come up; device in a safe-but-stuck state (2026-11-01)

After flashing root1+boot1 and setting bootcmd B-first, a cold boot left the
panel DARK and no network. root1/boot1 verified fine on the bench, so the modern
kernel either hung early from NAND or booted headless (u-boot passes
`console=ttyO0`, not tty0, and the image has no WiFi config, so no output and no
reachability). Stock A reappeared on neither WiFi nor the panel = u-boot did NOT
fall through to nandboot0, i.e. the modern uImage LOADED and bootm ran it.

**Device is SAFE, not bricked:** stock A (boot0/root0) untouched and read-only;
USB peripheral-boot (DFU) confirmed working (OMAP ROM 0451:d00e appears on the
seat-on-base + hold-ring gesture at power-up). But every recovery route needs a
controllable session, and the only channel that bypasses the hung B is DFU,
which needs a U-Boot payload we have not built.

**Recovery = build a J49 U-Boot SPL and push it over DFU.** The push tool sends
ONE image that the ROM runs from SRAM (~60 KB), so the payload is an SPL that:
(1) inits this board's SDRAM, (2) performs recovery, ideally
`nand erase 0x340000 0xc0000` (env0+env1) + reset -> stock u-boot in NAND then
falls back to its built-in default env (A-first) and boots stock A. Erase needs
no ECC, sidestepping the stock-vs-mainline BCH4 layout difference.

SDRAM: the authoritative timings are in the stock x-loader (backup mtd1_ipl.bin):
GP image, size 0x6c00 (27648 B), load 0x40200800; SDRC base built as an inline
immediate (not a literal), so extraction needs a disassembly. u-boot's omap3
SDRC auto-config may also bring up the 64 MB mobile DDR; to be validated.

Until the recovery u-boot exists, a normal power-on loops on the hung B (and the
backplate cold-resets every ~29 min, draining battery) -> power the unit OFF
(take it off the base) to park it safely.

### Why the B-slot modern kernel looked "hung" via u-boot bootm (2026-11-01, analysis)

Not a kernel hang -- a HEADLESS boot. The cmdline stock u-boot's nandboot1 builds is:
`root=/dev/mtdblock9 rootfstype=jffs2 console=ttyO0,115200 nlmodel=Display-2.14`
(from `setenv bootargs root=... ; run addjffs2 addtty addmodel`), then our
`CONFIG_CMDLINE_EXTEND` appends `root=/dev/mtdblock8 rootfstype=jffs2 rootwait`.

- The ONLY `console=` is `ttyO0` (OMAP UART1, no pads wired on the J49) -- and
  crucially NOT `console=tty0`. So the kernel never makes the panel VT a console,
  nothing writes to it early, omapdrm/panel never gets a modeset, and the LM3530
  backlight stays off -> the panel is BLACK even on a fully successful boot. Every
  kexec test passed `console=tty0`, which is the only reason the panel lit there.
- The NAND rootfs (root1) has NO WiFi provisioning (wpa conf / SSH key / NVS live
  only in the scratchpad overlay, never written to root1) -> no network either.
  Headless + no net = looks dead, but it is very likely running.
- `root=`: the last one wins, so it roots on mtdblock8 = root1 (correct). The DT
  has a valid memory@80000000 (64 MB), and the uImage loaded + bootm ran it (u-boot
  did NOT fall back to nandboot0), so the handoff itself worked.

Confirm + fix for the next B attempt:
1. Give the NAND boot `console=tty0` so the panel shows the boot log (and `getty@tty1`
   a login). Do it per-boot via u-boot: `fw_setenv addtty 'setenv bootargs
   ${bootargs} console=tty0'` (or add console=tty0 to the bootargs), NOT by baking
   console=tty0 into CONFIG_CMDLINE -- the squashfs/kexec test path deliberately uses
   console=ttyGS0 because rendering every printk on the 320px panel at 300 MHz is slow
   (first-light jump 80).
2. Provision root1 before the boot (wpa_supplicant-wlan0.conf + root/.ssh/authorized_keys
   + lib/firmware/ti-connectivity/wl127x-nvs.bin) so it joins WiFi and is reachable by
   SSH -- then dmesg confirms running-vs-hung directly. The device NVS and the overlay
   tree are in the session scratchpad (never git).

### Env erased -> stock boots but no WiFi; need stock-ECC env WRITE (2026-11-04)

The env-erase "recovery" (patch 0002) was wrong. Stock u-boot 2012.10's
compiled-in default env IS complete and A-first (nandboot0 nboot 0x400000,
root=mtdblock7), so an erased env DOES autoboot stock A to the app UI (confirmed:
blue home icon on panel). BUT the factory env also holds runtime vars the Nest
app reads: `nlwirelessregdom=A1`, `nldestcountry=FR,BE,NL`, `nlmodel=Display-2.14`,
`brightness=108`, plus nlCredentials*/pairing (device secrets). With the env
erased, the app comes up but **WiFi never associates** (no regdom) and the stock
**USB-ethernet gadget doesn't start** -> device is unreachable (no ping/SSH/USB).

Access now: only the OMAP ROM DFU path works. The fix: write the factory env back
to NAND env0 in **stock BCH4 ECC** (raw GPMC syndrome, OOB 36..63) -- the one thing
neither mainline u-boot nor the modern kernel can produce. Decoded factory env
(from backup mtd4_env0.bin, data-only, no OOB):
- image = 0x20000 (1 block): CRC32(LE,4) + flags(0xfb) + NUL-sep vars + 0x00 fill
- CRC = crc32(data[5:0x20000]) = 0x3f7ee3c6 (verified)
- meaningful prefix = 1862 bytes; rest 0x00; partition blocks 2..3 are 0xFF
Plan: DFU SPL (NEST_SPL_ENV_WRITE) inits DRAM, builds the 0x20000 image in DRAM
from the embedded 1862-byte blob + zero-fill, erases env0+env1, writes env0 with
GPMC BCH4 ECC at OOB 36..63 (no erased-page fold), warm-resets. Then stock boots
with full env -> WiFi back -> resume proven kexec/fw_setenv workflow. Bounded risk:
bad ECC -> env CRC fails -> default env -> stock A degraded (current state), re-erasable.

Modern-kernel /proc/mtd numbering (from nandprobe-83): env0=mtd3, env1=mtd4,
boot0=mtd5, root0=mtd6, boot1=mtd7, root1=mtd8. Stock /proc/mtd: env0=mtd4 etc.

### Env-WRITE SPL VERIFIED on the device: stock ECC is bit-exact (2026-11-04 14:34)

Pushed `tools/uboot-recovery/out/u-boot-spl.bin` (NEST_MODE=write) over DFU:
ROM gone 3 s after the push (SPL ran through erase+write+warm reset), stock
gadget back on USB 9 s later, stock A booted with the FULL factory env. Checked
over SSH on stock (root@<device>, `/usr/sbin/fw_printenv`,
`/usr/sbin/nanddump`):
- `fw_printenv` = all 40 factory vars (regdom A1, nlmodel, credentials, brightness);
  `/proc/cmdline` carries `nlmodel=Display-2.14 brightness=108`.
- `nanddump -o -l 0x20000 /dev/mtd4`: **ECC failed 0, ECC corrected 0**, md5
  `6efa009b…` == the expected image built from the SPL blob (data-only). So the
  SPL's GPMC BCH4 register sequence + 7-byte MSB-first packing at OOB 36..63 with
  no polynomial fold is exactly what the stock kernel/u-boot expect. OOB page 0 =
  `09ed1ae8a13280060e12361f969073cf95b6bce0c0bc823919ae8d50`; all-zero pages get
  all-zero ECC (raw remainder, confirms no fold). OOB 0..35 = 0xFF. env1 erased.
- The Nest app came up in the setup wizard ("fully reset") because the env had been
  empty on the previous boot; WiFi scan works again once regdom is back.

**Correction to the previous section:** "stock USB gadget doesn't start with an
erased env" was a misobservation -- the display was plugged into the wrong USB hub.
The mass-storage gadget (2464:0002) is up on every stock boot; the only real
symptoms of an erased env are no WiFi (regdom) and the app's setup wizard.
SSH over WiFi is the only channel into stock; USB is mass-storage only.

We now have a proven **write-with-stock-ECC primitive** in the SPL (any block,
any content) -- the building block for writing boot1/env from DFU without stock.

### B-slot ROOT CAUSE: uImage load address == nboot address (2026-11-04 16:47)

The "headless/hung" B boot was neither. Traced stock u-boot through a self-
reverting one-shot `bootcmd` that records progress in the env (`setenv probe N;
saveenv` after each step) and a zero-filled DRAM ring:
- `probe=3` and no `bootmret`: `nboot.i` succeeded, bootargs set, `bootm` ran and
  never returned; the ring had no decompressor breadcrumb 'A' and was filled with
  0xFF-ish garbage; stock A was back ~8 s after the reboot (u-boot exception ->
  immediate reset -> A-first).
- Our boot1 uImage had **load=ep=0x81000000 = `${loadaddr}`**, the address `nboot`
  loads the whole uImage (header included) to. u-boot 2012.10 `bootm` then takes the
  **XIP** path (no copy) and jumps to `ep` = the 64-byte uImage HEADER, not the
  zImage -> garbage code -> exception -> reset. With `bootcmd` B-first that is an
  endless crash loop (black panel, never stock) = the 2026-11-01 "hang".
- Stock's own uImage: `load=ep=0x80008000` (backup mtd6_boot0.bin). Fix: build ours
  the same way (`mkimage -a 0x80008000 -e 0x80008000`); bootm then copies the
  zImage(+DTB) to 0x80008000 and jumps there.

Tracing recipe (reusable): `fw_setenv bootcmd 'setenv bootcmd "<A-first>"; setenv
probe 1; saveenv; <step> && setenv probe 2 || setenv probe fail; saveenv; ...'`,
`reboot -f`, read `fw_printenv probe` after stock returns. Note the ramoops zone at
0x83e00000 is overwritten by stock's framebuffer on every stock boot (320x320x2 =
~200 KB of panel pixels), so only the DEBUG_LL ring at 0x83e80000 survives a round
trip through stock. Also: stock's PATH lacks /sbin -> use `/sbin/reboot -f`,
`/sbin/fw_setenv`; a plain `reboot` silently does nothing (rc=127).

### Stock u-boot bootargs limit: fixup_silent_linux() overflows at 256 bytes (2026-11-08)

Two one-shot B boots with long bootargs (`systemd.run=...` on 11-04, `systemd.debug_shell=... loglevel=8
ignore_loglevel earlyprintk ...` on 11-07) fell straight to stock A within ~8 s. Env-probe tracing showed
`nboot` succeeded (`rc_nboot=0`) and **`bootm` returned with `rc_bootm=1`**. Cause: the stock env has
`silent=1`, so u-boot 2012.10 runs `fixup_silent_linux()` (common/cmd_bootm.c) on every bootm. It copies
`bootargs` into a **`char buf[256]`** on the stack to blank the `console=` value (or append ` console=`);
anything longer smashes the stack and bootm fails. Rules for the B slot:

- total `bootargs` after `run addjffs2 addmodel` (` rootfstype=jffs2 nlmodel=Display-2.14`, 38 bytes) plus
  the appended ` console=` (9 bytes) must stay **< 256**, i.e. our own part <= ~200 bytes;
- the kernel always sees `console=` (ttynull) from u-boot, so the panel console must come from elsewhere;
- the DTS `/chosen bootargs = "console=tty0"` is DISCARDED on a u-boot boot unless the kernel has
  `CONFIG_ARM_ATAG_DTB_COMPAT_CMDLINE_EXTEND=y` (the default `CMDLINE_FROM_BOOTLOADER` replaces it with the
  ATAG cmdline; verified from `/proc/cmdline` of the 11-07 B boot). Added to `j49.cfg`, needs a kernel rebuild.
  Deleting `silent` from the u-boot env would also work but changes stock A's console (ttyO0) and u-boot's
  serial output, so it was not done.

Also relevant for unattended bring-up: `bpd.service` is in `sysinit.target.wants` with `DefaultDependencies=no`,
so it runs even when sysinit stalls -> the backplate watchdog does NOT reset a stalled B boot; a stalled B stays
up until a power-cycle (11-04: 69 h). Bring-up cycles therefore use `omap_wdt.early_enable=1
omap_wdt.timer_margin=300` plus a temporary `/etc/systemd/system.conf.d/10-nowdt.conf` (`RuntimeWatchdogSec=0`)
on root1 so the HW watchdog always warm-resets to stock within 5 min, and a `nest-early.service`
(`DefaultDependencies=no`) that dumps `systemctl list-jobs`/sysrq-w/journal/dmesg to `/root/early-*.txt` after
120 s and reboots. Both must be removed once B reaches multi-user.

### B-slot "stall" root cause: GPMC was disabled in the DTS (2026-11-08)

All B-slot kernels built after 11-01 waited forever at `rootwait` because `&gpmc { status = "disabled"; }`
(left over from the kexec/initramfs era) removed every `/dev/mtd*`; the only DTB that reached userspace
(nand15, 11-01) still had GPMC enabled. Proof: identical kernel with `rootdelay=5` instead of `rootwait`
panicked ("Unable to mount root fs") within ~10 s. The pinmux hog was never the stall; it is required
(backlight, MMC2/WiFi, I2C2/3). Fixed: GPMC `status = "okay"`. Bring-up rules learned: diff every new DTB
against the last known-good one (`dtc -I dtb -O dts`); never use `rootwait` on a headless bring-up boot
(use `rootdelay=N` + `panic=`), and remember that `omap_wdt.early_enable` only resets the board when
`CONFIG_WATCHDOG_HANDLE_BOOT_ENABLED=n`. Also: the DRAM post-mortem window 0x83e00000..0x84000000 is
overwritten by stock's framebuffer/VRAM on every stock boot since the 11-04 factory reset, so ramoops and
the DEBUG_LL ring cannot be read back after a return to stock; the bring-up kernels use `mtdoops` into an
`oops` partition carved from the tail of boot1 (boot1 shrunk to 5 MB in the modern DT; stock unchanged)
plus a tiny `nest_deadman` driver (panic after N s unless userspace clears it).

### Running code on a B boot without network: init hook + NAND payload (2026-11-08)

With B at multi-user but no WiFi, the only hands-free channel into the modern kernel is the kernel
command line. The bring-up kernels end `CONFIG_CMDLINE` with
`init=/bin/sh -c "mount -t devtmpfs dev /dev; mount -t proc proc /proc; mount -t sysfs sys /sys;
nanddump -n -s 262144 -l 1048576 -f /tmp/p /dev/mtd14 > /root/hooklog 2>&1; cd /; tar xf /tmp/p ...;
sh /root/x ...; sync; exec /sbin/init"`. The payload tar is written from stock with
`nandwrite -p -s 0x540000 /dev/mtd8 payload.tar` (stock boot1 is 8 MB; in the modern layout that is
`oops` = mtd14 + 0x40000). Rules learned the hard way:
- `init/main.c` turns every boot argument containing `=` into an init environment variable and drops
  every argument containing `.` as an "unused module parameter", so the hook text may contain neither;
  use `$(printf '\056')` for dots and `\075` for `=` (doubled backslashes inside the Kconfig string).
- `mtdoops` erases block 0 of its partition on attach when it finds no valid record, so a payload at
  offset 0 of `oops` is lost; +0x40000 (block 2) survives. The raw read (`nanddump -n`, no `-o`) ignores
  stock's BCH ECC layout; busybox tar complains about the trailing 0xFF but extracts.
- Output goes to files on root1 (`/root/x.txt`, `/root/hooklog`), read back from stock with a raw root1
  dump (`nanddump -n -o -i /dev/mtd9`) + `jffs2cat`. `nest-diag.service` on root1 still warm-reboots B
  after 300 s without a wlan0 address, so every cycle returns to stock on its own (~7 min).
- Busybox on root1 lacks `timeout`, `devmem`, `head -N` and `od -A`; `i2cget` needs `-f` because the
  twl driver owns 0x4b; `/run` is a 9.7 MB tmpfs, so journald needs `RuntimeMaxUse=2M` or it fails with
  "No space left on device"; do not use `systemd.log_level=debug` (floods the 256 KB log buffer).

### B-slot WiFi root cause: CONTROL_DEVCONF1.MMCSDIO2ADPCLKISEL (2026-11-13)

Every cold (u-boot) boot of the modern kernel logged `mmc1: Failed to initialize a non-removable card`
at 1.6 s, so the WL1271 never appeared and B had no WiFi, while kexec boots from stock worked.
Candidates eliminated with the init-hook experiments: pad mux (identical to stock, applied at 0.58 s,
before the 1.48 s mmc probe), WLAN_EN/gpio103 sequencing, the TWL4030 VMMC2 LDO (already at 1.8 V on a
cold boot, same as stock), sys_clkout1 (off in stock too). Stock's register dump showed
`CONTROL_DEVCONF1 (0x480022d8) = 0x40`: bit 6 = MMCSDIO2ADPCLKISEL, the MMC2 "internal loopback" input
clock. A cold boot leaves it 0; a kexec inherits stock's 1. Setting the bit from userspace and rebinding
`480b4000.mmc` gave `mmc1: new SDIO card at address 0001`, `wlcore: loaded` and a DHCP lease on wlan0
within seconds (b25, 2026-11-13 15:56). Fix: kernel patch
`0007-arm-omap2-pdata-quirks-nest-j49-mmc2-loopback-clock.patch` adds `{ "nest,j49",
hsmmc2_internal_input_clk }` to `arch/arm/mach-omap2/pdata-quirks.c` (the Nokia N9/N950 need the same
for their WL1271). Side findings still open: VAUX2 (panel vcc) runs at 1.8 V although the DTB asks
2.8 V and stock uses 2.8 V; the panel works regardless. Operational lesson: never `memdump` whole
peripheral pages on stock (a 4 KB read of 0x48002000 rebooted it); read single registers.

### Second WiFi blocker on a cold boot: the NVS is requested before / is mounted (2026-11-13)

With the DEVCONF1 quirk in the kernel (b26) the WL1271 enumerates at 1.6 s, but `wl12xx` is built in
and probes immediately: `Direct firmware load for ti-connectivity/wl127x-nvs.bin failed with error -2`
(no root filesystem yet), later `wlcore: ERROR NVS file is needed during boot` / `firmware boot failed
despite 3 retries`, wlan0 shows MAC 00:00:00:00:00:01 and never associates. The b25 run only worked
because its rebind happened at 23 s, after the root mount. Fix shipped in `j49-config`:
`j49-wlan-reprobe.service` (DefaultDependencies=no, after local-fs.target, before network-pre.target)
unbinds/binds `wl12xx.2.auto` when wlan0 still carries the no-NVS MAC, so the probe repeats with the
NVS available. The standard alternative is building wlcore/wl12xx/wlcore_sdio as modules (they would then
load from the rootfs); that needs the Yocto image rebuild and a root1 reflash, so the unit is the interim.
Also shipped: `10-j49-journald.conf` (RuntimeMaxUse=2M) because /run is a 9.7 MB tmpfs.

### Re-probe unit v2 (2026-11-14)

The first unit only did `unbind`/`bind` and returned. That is not enough: wlcore registers `wlan0`
from the *asynchronous* NVS `request_firmware_nowait()` callback, so the `bind` write returns before
the new `wlan0` exists and `wpa_supplicant@wlan0` / `systemd-networkd` (ordered `After=` the unit)
can start against the old, half-dead interface or none at all. In b26p the unit was only ever
exercised as a no-op (the bring-up hook had already rebound the device), so this was not caught
before the b27 B-first boot went silent. v2 moves the logic to `/usr/libexec/j49-wlan-reprobe`:
rebind, then wait up to 20 s for `wlan0` to come back with a MAC other than `00:00:00:00:00:01`
(plus `udevadm settle`), and fail the unit visibly if it does not. The unit is also wanted by
`multi-user.target` so it does not depend on something else pulling in `network-pre.target`
(`systemd-network-generator` happens to, but that is incidental).

Note for a B slot without network: the modern rootfs has no `fw_setenv`, so switching the stock
u-boot back to A-first from inside B is not possible without crafting the env block by hand; the
USB env-write SPL or a stock window remain the way back.

### Serial console on the physical UART + stock u-boot (2026-11-17)

The board's debug UART is OMAP **UART1** (MMIO 0x4806a000), 115200 8N1, the same
port stock u-boot and the stock 2.6.37 kernel use (stock env: `stdout=serial`,
`addtty=setenv bootargs ${bootargs} console=ttyO0,${baudrate}`). The mainline
`8250_omap` driver names it **ttyS0** (not ttyO0). Three pieces make a full
u-boot-through-userspace console on one wire:

1. **Kernel log + login (modern):** `j49.cfg` adds `console=ttyS0,115200n8` to
   `CONFIG_CMDLINE`, placed *before* `console=tty0` so the panel stays the primary
   `/dev/console` and ttyS0 is a secondary console; `systemd-getty-generator` then
   starts `serial-getty@ttyS0` automatically. Harmless when nothing is attached.
2. **Pin mux (modern):** the UART1 TX/RX pads are added to the DTS pinmux hog
   (`uart1_tx` 0x014c = 0x0000 output mode0, `uart1_rx` 0x0152 = 0x0118 input
   pullup mode0 — the dedicated pads stock also muxes, balls W7/V7). Without this
   the controller probes (ttyS0 appears) but the signal never reaches the pads.
3. **Stock u-boot output:** stock u-boot already writes to UART1 (`stdout=serial`)
   but the env has **`silent=1`**, which suppresses it (and triggers the 256-byte
   `fixup_silent_linux` bootargs limit). Clearing `silent` makes u-boot verbose on
   every boot and removes that limit. The modern rootfs has no `fw_setenv`, so clear
   it either from a stock window (`/sbin/fw_setenv silent`) or by writing an env
   block without `silent` via the env-write SPL (stock BCH4 ECC); writing the env
   straight from the modern kernel's MTD is only safe once its NAND ECC is confirmed
   to match stock, otherwise u-boot falls back to its built-in (A-first) default env.

Hardware still required: a 3.3 V (NOT 5 V) USB-TTL adapter on the UART1 TX/RX/GND
test points, which still need locating on the diamond PCB (balls W7 = uart1_tx,
V7 = uart1_rx). Until then the USB gadget console (`ttyGS0`, CDC-ACM) already gives
a login over the micro-USB cable; add `console=ttyGS0` to the cmdline if the kernel
log over USB is wanted too (u-boot cannot use the USB gadget, so UART1 is the only
way to see the bootloader).

## Planned: one ECC scheme (BCH8) for U-Boot and Linux

Not done on 28 November 2026; the reason below still holds, and it is why a kernel update
needs USB. One point is overtaken: since 27 November the kernel `uImage-j49-a` starts `nandio`
first, and `nandio` can choose the root slot from a record in `env1` (see the top). Two kernel
files are no longer the only way to choose a slot.

State on 2026-11-26. U-Boot reads and writes NAND with BCH8 (`NAND_OMAP_ECCSCHEME_BCH8_CODE_HW_DETECTION_SW`),
the kernel with `ti,nand-ecc-opt = "bch4"`. Neither can read what the other wrote. Consequences:

- U-Boot has no usable environment on flash (env0 "never read back successfully"); the
  compiled-in `nest_j49.env` is the environment, with `root=/dev/mtdblock6` fixed in it.
- Linux cannot change U-Boot settings (`fw_setenv` would write BCH4) and cannot write the kernel:
  boot0 has to be written through the U-Boot DFU window.
- The root slot is therefore chosen by WHICH kernel sits in boot0: the kernel appends its built-in
  command line after U-Boot's and the last `root=` wins. That is why there are two kernel
  artifacts, `uImage-j49-a` (root0) and `uImage-j49-b` (root1), identical apart from the built-in
  command line and the `read-only` flag of root0 in the device tree.

Moving the kernel to BCH8 (the same scheme as U-Boot) would allow ONE kernel, the slot as a
U-Boot variable set from Linux, and kernel updates without the DFU window. Cost and order:

1. Proof first, on an empty partition: write with Linux `bch8`, read with U-Boot, and the other
   way round. Whether the two layouts match byte for byte is NOT tested.
2. Everything Linux wrote has to be rewritten in the new layout: both root partitions and the
   scratch partition (which holds the WiFi credentials and the NVS under `j49-persist/`).
3. U-Boot has to be reflashed (spl0/spl1): `root=` must leave the compiled-in environment and
   the environment must be read from flash. A failure there needs the ring gesture and the
   ROM-push recovery, i.e. somebody at the device.

Not done: it is a change to the boot chain for maintainability only. Do it when U-Boot has to be
reflashed for another reason.
