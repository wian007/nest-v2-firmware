# dfu-install -- a DFU installer for our OS on the Nest J49

## Status (2026-11-23) -- read this first

**The whole chain works on hardware: our U-Boot in NAND boots our Linux 6.6 + Yocto rootfs from NAND by itself.**
Stage 1 (spl0/spl1), stage 2 (root0, verified byte for byte) and stage 3 (boot0) are done; `reboot` in Linux
comes back into Linux (console + login on the panel, `root@nest-j49`, jffs2 root0 mounted rw).

What had to be fixed after the transport worked (all in the repo, details in `CHANGELOG.md`):

| symptom | root cause | fix |
|---|---|---|
| `bootz` reset the board, nothing ran | `CONFIG_HUSH_PARSER` off: `bootz addr \|\| reset` passed `\|\|`/`reset` to bootz as ramdisk/FDT address | `CONFIG_HUSH_PARSER=y` |
| `LZMA data is corrupt` | U-Boot enabled the OMAP3 L2 cache | `CONFIG_SYS_L2CACHE_OFF=y` |
| installer `open(/dev/mtd6)` EACCES | production DTB has root0/boot0 `read-only` | installer DTB variant (`installer/make-installer-dtb.sh`) |
| staging magic `0x0000` | kernel reused the DFU staging window as free RAM | `staging@82000000` reserved-memory in the installer DTB |
| `MEMERASE64` ENOTTY | ioctl request declared `u64` on ARM32 | `u32` in `mtdflash.rs` |
| staging check failed | jffs2 LE magic is bytes `85 19` | accept both byte orders |
| last write EINVAL | last chunk not a multiple of the 2048 B page | pad with 0xFF |
| root mount verify OOM | 46 MB jffs2 mount on 64 MB RAM | `--verify-mem` (byte compare against the staging image) |
| Nest dead after the installer's `reboot -f` | kernel restart leaves the board unusable until a hard reset | installer resets through the OMAP watchdog (`wdtreset`) |
| production kernel rooted on root1 | built-in cmdline (`root=/dev/mtdblock8`) is appended last and wins | `installer/build.sh` sets `root=/dev/mtdblock6` |
| jffs2 `-EROFS` / oops in `jffs2_flush_wbuf_gc` | root0 `read-only` in the production DTB | A-slot DTB (`make-installer-dtb.sh ... a-slot`): root0 writable, boot0 protected |

### Boot policy and RAM flags (`uboot/files/nest_j49.env`)

`bootcmd` = `run nandboot || run usbinstall` with console output on `nulldev` (no UART). Two ONE-SHOT flags in DRAM at
`0x83e00000` (write with `dfu-util -a ring -D`, or from Linux through /dev/mem before a watchdog reset):

- `0xac0ac0ac`: debug console on `/dev/ttyACM0` (USB CDC-ACM, U-Boot prompt `nest #`) instead of booting
- `0xdf00df00`: stay in the DFU window (stage 2/3) instead of booting boot0

`nandboot` arms WDT2 (~120 s) just before the kernel starts: a kernel that hangs early resets back into U-Boot and the
DRAM debug ring survives (`dfu-util -a ring -U`, decode with `tools/kernel-firstlight/decode-postmortem.py`).

### USB console from Linux (works after a plain cold boot)

`/dev/ttyACM0` on the host is a root shell (`root@nest-j49`, getty on `ttyGS0`). Two things were needed:

- `nestusb 1500` in `nandboot` (U-Boot patch `files/0003-cmd-nest-usb.patch`) brings the USB gadget/PHY up the way the
  console/DFU paths do. Without it the kernel's musb reports `VBUS_ERROR in b_idle (<SessEnd)` after a cold boot.
- `nandboot` clears the first 4 KB of the DEBUG_LL ring (0x83e80000) before booting. The kernel's DEBUG_LL ring backend
  takes its write index from that header; random leftovers (e.g. the stock rescue kernel's framebuffer, which lives in
  the same DRAM) made the decompressor write to a bogus address and hang. That was behind every "Nest dead after
  rescue flash / after a reset" symptom. (A proper fix would validate the index in `ramring.S`.)
- `wdtarm` first switches on WDT2's fck/ick (CM_*CLKEN_WKUP bit 5): they are off after a stock-kernel reboot and a
  register write would hang the bus.
- Progress markers: `0x83ef0f00` = 0xb1 start, b2 nand read ok, b3 USB primed, b4 WDT armed (read with `dfu-util -a ring -U`).

`reboot` in Linux comes back into Linux with the console, by itself.

### Open points

- Rootfs content: `wlcore` firmware missing (`ti-connectivity/wl127x-*`), four jffs2 `wrong data CRC` notices at
  0x566b54..0x56983c (benign so far).
- The ring gesture (ROM USB) is only needed to rescue-flash spl0/spl1 or to recover a bad U-Boot.

### Tooling notes (no UART needed)

USB ACM console (`nest #`), DRAM ring/ramoops readback, webcam on the panel (reflections look like a lit screen: look for
the installer UI), `uhubctl -e -l 5-1.4 -p 1` (port power; does NOT reset a hung Nest -- use the ring).

### How to run a push (operational rules learned the hard way)

- **Start the loader BEFORE the DFU gesture.** The ROM window after
  power-up is short (3-30 s). Order: start `session1-stock.sh` (it waits),
  then power the unit off, seat it on the base and hold the ring 10-15 s.
- A device looping into `d00e` (e.g. our broken U-Boot) can be pushed to
  without a new gesture; a silent, black, USB-less device needs a power
  cycle plus the gesture. A stale `d00e` that stops answering needs a full
  power-off (take the unit off the base).
- If a loader never pushes although `d00e` is visible, kill it and
  restart it. Do not trust `tail` of its log too early: it prints
  `successfully transfered N files` only at the end.
- `0451:d00e` = ROM, `0525:a4a7` = rescue kernel serial gadget (`/dev/ttyACM0`,
  root shell, 115200, `stty -F /dev/ttyACM0 raw -echo`), `2464:0002` = stock,
  `0451:df10` = our U-Boot DFU (target). Check the USB hub/cable first if
  nothing shows (see the project memory).
- Rescue shell: `/proc` is not mounted (`mount -t proc proc /proc`); send
  short commands one per line; there is no `od`/`nanddump` working in the
  *unmodified* rescue kernel (`dd | hexdump -C` works).
- Never push a payload much larger than ~7.3 MB: the rescue kernel has an
  uncompressed 20 MB initramfs on a 64 MB board; the 3 MB larger variant
  never reached userspace.

### Flash our U-Boot / restore stock (both proven)

    # our U-Boot into spl0+spl1 (default payload, nestinstall runs by itself)
    installer/build-stock-kernel.sh                 # -> payload/nest-install-uImage
    session1-stock.sh                               # loader first, then the gesture

    # back to stock (spl0/spl1 from the backup, truncated to the real image)
    B=/path/to/your/backup
    head -c 245176 $B/mtd2_spl0.bin > /tmp/spl0.trunc   # 64 + size field; rest is 0xFF
    head -c 245176 $B/mtd3_spl1.bin > /tmp/spl1.trunc
    UBIMG=/tmp/spl0.trunc UBIMG1=/tmp/spl1.trunc \
      OUT=$PWD/payload/nest-restore-uImage installer/build-stock-kernel.sh
    KERNEL=payload/nest-restore-uImage ./session1-stock.sh

The env0 partition was not touched by our U-Boot (it never ran far enough
to `saveenv`), so no env restore was needed.

### What was wrong with the first attempts (fixed in `build-stock-kernel.sh`)

1. initramfs extracted as a normal user -> all 97 device nodes dropped, no
   `/dev/console`/ttyGS0. Now extracted and repacked as root in docker, with
   a node-count check.
2. `libgcc_s.so.1` missing in the stock initramfs -> `flash_eraseall` and
   `nandwrite` did not start. Now copied from the 2008q3 toolchain, with a
   check of every NEEDED lib.
3. `nestinstall` was appended after `${INITDIR}/start`, which blocks `rcS`.
   Now inserted before `start`; on failure it waits 120 s so the shell
   stays readable.
4. `rm linux/usr/initramfs_data.*` deleted the tracked `initramfs_data.S`.
   Now only generated files are removed.

### Recovery

The ROM USB boot is the un-brick path and was verified several times,
including with both spl slots holding our non-booting U-Boot. Hold the ring
10-15 s after a full power-off; push `nest-restore-uImage` as above.

### Build commands

    uboot/build.sh
    installer/build.sh ~/Projects/nest-yocto/build/tmp/work-shared/nest-j49 \
      ~/Projects/nest-yocto/build/tmp/deploy/images/nest-j49/j49-image-nest-j49.rootfs.jffs2
    installer/build-stock-kernel.sh      # Docker, stock 2.6.37, ~5 min

Host deps: `dfu-util` (nix shell), udev rule for `0451:df10` (NixOS
`extraRules` in `~/nixos/modules/electronics.nix`), Docker,
`tools/omap-loader/out/omap_loader`.

## Design

One ROM-DFU gesture plus two plain-USB sessions, and the modern Yocto OS
owns the A slot. Stock fallback is deliberately given up (the ROM USB
window is the un-brick safety net, plus `restore-stock` from the backups).

Read `docs/flash-layout.md` first; the partition numbers below come from
there (modern-kernel numbering).

## Boot chain after install

    ROM -> x-load (ipl, stock, untouched -- loads u-boot from spl0)
         -> U-Boot 2024.07 "nest-j49" (spl0/spl1, ours, stock BCH4 ECC)
         -> our kernel uImage (boot0, u-boot's own ECC)
         -> root0 jffs2 (our j49-image, mainline bch4, kernel-written)

Every partition has exactly one writer and one reader, so no ECC ever
crosses formats (the lesson of docs/flash-layout.md):

| partition | writer                      | reader           | ECC            |
|-----------|-----------------------------|------------------|----------------|
| ipl       | -- (never written)          | ROM              | --             |
| spl0/spl1 | flashstub (session 1)      | stock x-loader   | stock BCH4     |
| boot0     | our u-boot (session 3 DFU) | our u-boot       | u-boot's own   |
| env0/env1 | left stock; `saveenv`      | our u-boot       | u-boot's own  |
| root0     | installer kernel (session 2) | our kernel    | mainline bch4 |

The kernel never reads boot0 and u-boot never reads root0, so the
stock-vs-mainline BCH4 split never has to be reconciled. The existing B
slot (root1, mainline bch4) keeps working until it is re-populated.

## The three sessions

**Session 1 has two implementations.** The default is the proven stock-kernel
rescue (`session1-stock.sh`); the custom bare-metal `flashstub/` is kept as
experimental work (it hung in its earliest bring-up without UART visibility and
needs the beep/WDT diagnostics finished).

1. **`session1-stock.sh`** -- one ROM-DFU gesture (hold the ring 10-15 s,
   `0451:d00e`). Pushes the nest-dfu-attack chain: x-load + stock u-boot +
   `payload/nest-install-uImage` (STOCK 2.6.37 rescue kernel with our
   initramfs). That kernel runs `nestinstall`, which writes our U-Boot
   uImage into **spl0+spl1 through the stock kernel's NAND driver** --
   i.e. in STOCK BCH4 ECC, exactly what stock x-loader reads (the same
   primitive that flashed boot1 on 2026-11-01). Progress shows on the
   panel (logo + `/dev/tty1` text) and on the USB gadget console. Warm
   reset -> stock x-loader loads OUR U-Boot -> boot0 is invalid -> DFU
   gadget `0451:df10`.
   Build the payload with `installer/build-stock-kernel.sh` (Docker + the
   attack repo's 2008q3 toolchain; only the initramfs + link rebuild).
   The bare-metal `flashstub/` remains as an alternative for session 1;
   see its headers for status and the beep/WDT diagnostics.
2. **`stage2.sh <installer-kernel> <jffs2>`** -- no gesture, plain USB.
   The device is in our U-Boot, whose `bootcmd` fell through to `dfu 0`
   (gadget 0451:df10) with all three alts in ONE window. Two downloads:
   `-a installer` pushes the small installer kernel to RAM @0x80A00000,
   `-a rootfs` pushes the **raw jffs2** to RAM @0x82000000 (22 MiB
   window; the image is nearly incompressible -- 22 MB -> 21.5 MB gz --
   so it cannot ride inside the kernel's initramfs on a 64 MB board).
   Detach makes U-Boot `bootz` the kernel; its init streams the staging
   image straight from `/dev/mem` into `/dev/mtd6` (`nest-mtdflash
   --from-mem`, kernel-side ECC/bad-block handling, no tmpfs copy),
   erases first with exact per-block progress, shows the steel INSTALLER
   screen, verifies by mounting the jffs2, and reboots.
3. **`stage3.sh <kernel-uimage>`** -- no gesture. The device is back in
   the same DFU window (`boot0` still empty/invalid). `dfu-util -a boot0`
   writes the kernel uImage with **this U-Boot's own ECC**
   (`nand_write_skip_bad`); then four junk bytes go into the RAM alt so
   `bootz` cannot boot the stale stage-2 kernel again (DRAM survives a
   warm reset -- without the junk write the installer would loop);
   detach -> `bootz` fails -> `reset` -> `nandboot` boots the OS.

The chain is self-healing: `bootcmd=run nandboot || run usbinstall`, and
`usbinstall` ends in `bootz ... || reset`, so any failed kernel boot lands
the device back in the DFU window instead of a dead prompt.

`restore-stock.sh` (flashstub based, **unproven -- the flashstub hangs**)
was meant to put the ORIGINAL chain back. Use the rescue-kernel restore
from the Status section instead (proven 2026-11-21). The factory env step
(`tools/uboot-recovery` NEST_MODE=write SPL) is only needed if env0 was
rewritten, which our U-Boot never got far enough to do.

## U-Boot (`uboot/`)

`uboot/build.sh` builds U-Boot 2024.07 from the same tarball/toolchain as
`tools/uboot-recovery` and emits `out/u-boot-nest-j49.uimg`:
`CONFIG_TEXT_BASE=0x80100000`, `DEFAULT_DEVICE_TREE=omap3-nest-j49`
(UART1 console, TWL4030 on I2C1 @400 kHz, MUSB OTG, GPMC NAND), wrapped
exactly like the stock `mtd2_spl0.bin` (`mkimage -O u-boot -A arm
-T firmware -C none -a 0x80100000 -e 0`, header `27051956 / 11 02 05 00`;
the hexdump of the stock image shows os=17 u-boot, type=5 firmware,
entry=0 -- the x-loader jumps to the load address) so stock x-loader
loads it unchanged. The **whole default env** comes from
`files/nest_j49.env` (CONFIG_USE_DEFAULT_ENV_FILE): the stock env on NAND
is unreadable with our ECC, so every boot simply uses that file.

NAND ECC: U-Boot's BCH8 HW-calc/SW-detect scheme (`CONFIG_BCH`, no ELM on
OMAP3). boot0/env are written AND read by U-Boot only -- the kernel never
reads boot0, so no ECC format is ever exchanged. `restore-stock` needs the
stock env writer for the reverse direction, which already exists.

### Hardware notes / first-test knobs

- U-Boot proper after the stock x-loader runs `s_init()`: `prcm_init()`
  reprograms DPLLs with the J49 SDRAM values from the meta-j49 patches
  (the same double-init the original stock 2012.10 flow did), and
  `per_clocks_enable()` gates I2C/SPI/DSS clocks on. Watchdog is disabled
  there too.
- UART1 console is the physical W7/V7 pads @115200 8N1: attach a 3.3 V
  USB-TTL (never 5 V) to interact with the `nest #` prompt.


### Verified build result (2026-11-21)

`uboot/build.sh` produces `out/u-boot-nest-j49.uimg`, 523 KiB (spl0 slot is
1536 KiB), and checks the header itself:

    magic 27051956  load 80100000  ep 0  os 17 (u-boot)  arch 2  type 5 (firmware)  comp 0

-- byte-for-byte the stock mtd2_spl0.bin shape (`11 02 05 00` at 0x1C).
`.config`: TEXT_BASE=0x80100000, NAND_OMAP_ECCSCHEME=5 (BCH8 HW-calc/SW-detect),
BCH=y, CMD_DFU/DFU_OVER_USB/DFU_RAM/DFU_NAND/CMD_MTDPARTS (DFU_NAND needs
CMD_MTDPARTS; DFU_WRITE_ALT is only a fastboot helper and not needed),
USB_GADGET_DOWNLOAD + USB_MUSB_GADGET/OMAP2PLUS + TWL4030_USB,
USE_DEFAULT_ENV_FILE=nest_j49.env, ENV_IS_IN_NAND @0x340000/0x60000.

The appended DT compiles to the expected tree: `nest,j49` compatible,
64 MB memory, uart1 enabled with `pinmux_uart1_pins`, uart2/uart3
disabled, i2c1 (TWL) enabled, i2c2/3 + all mmc disabled, gpmc okay with
`nand@0,0 (ti,omap2-nand)`, usb_otg_hs with mode 3 + twl usb2_phy. The
default env strings (`bootcmd=run nandboot || run usbinstall` and the three-alt
`dfu_alt_info` (`installer` RAM, `rootfs` RAM staging, `boot0` raw NAND)
are embedded.

## Layout

- `flashstub/` -- the bare-metal session-1 second stage (C, no libc,
  integer-only, linked 0x80100000). `nand.c` is the stock-BCH4 writer;
  `panel.c` is the round-LCD bring-up + the steel installer screen; see
  the headers in each file for the register-fact sources.
- `ui/steel.h` -- the shared visual tokens (C mirror of
  tools/j49-screen; the palette ultimately comes from homelab-ui's
  `index.css`).
- `mkpayload.py` -- builds the NFD1 payload (header + image + block
  ranges) the stub receives at 0x82000000.
- `nest-dfu/` -- vendored runtime images from `~/Projects/nest-dfu-attack`
  (`x-load.bin`, stock `u-boot.bin`, `rescueUImage`, `uImage`) with
  provenance + sha256s; the runtime chain no longer needs the external
  tree (rebuilding the stock kernel still does).
- `uboot/` -- the U-Boot 2024.07 nest-j49 board (defconfig, DTS, whole
  default env, build -> `out/u-boot-nest-j49.uimg`). See its README
  section below.
- `installer/` -- the stage-2/3 kernel artifacts: `build.sh` takes a
  kernel-firstlight workdir OR the Yocto work-shared/nest-j49 dir plus
  the j49-image jffs2 and produces `out/uImage-j49-a` (production kernel,
  no initramfs, load/ep 0x80008000) and `out/zImage-installer` (same
  kernel WITH a small initramfs: busybox, `j49-screen`, `nest-mtdflash`,
  the stage-2 `init` templated with the image length, and NO jffs2 --
  the image stays at 0x82000000 in DRAM instead). `mtdflash.rs` streams
  into /dev/mtd6 via mtdchar (kernel ECC/bad-block handling) from stdin
  or from `/dev/mem` (`--from-mem`, staging magic checked) and publishes
  exact erase/write progress for the installer screen -- same no-crates
  raw-libc conventions as `ramoops-read.rs`.
- `stage1.sh`/`stage2.sh`/`stage3.sh` -- session orchestration.
- `99-nest-dfu.rules` -- udev: unprivileged access to 0451:df10 (our
  DFU gadget) and 0451:d00e (ROM peripheral boot).

## Payload format (NFD1)

    u32 magic "NFD1", u32 image_len, u32 n_ranges(1..4), u32 0,
    { u32 first_block, u32 n_blocks } * n_ranges,   <- written in order
    image bytes

The stub validates every range against its own allow-list as well, so a
corrupt or hostile payload cannot reach any partition outside spl0/spl1
by construction. The host builder checks the same list first so typos
die on the host.
