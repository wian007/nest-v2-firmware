# Modernizing the Nest J49 to a current Yocto stack

Goal: a fully modern image (recent LTS kernel, systemd, current glibc/poky) that
boots on the physical Nest Learning Thermostat 2nd gen (J49).

## Hardware (confirmed from the live device)

| Item        | Value |
|-------------|-------|
| Machine     | "Nest J49", board codename "diamond", revision 020e |
| SoC         | TI OMAP3 / Sitara AM3703 (likely; AM-DM37xx class), Cortex-A8 (part 0xc08), up to 1 GHz |
| ISA / ABI   | ARMv7-A, VFPv3 + NEON; stock userspace is armhf hard-float |
| RAM         | 64 MB (MemTotal 58472 kB); SDRAM base 0x80000000, loadaddr 0x81000000 |
| Display     | SPI panel s6d05a1 / tianma tm025zdz01 (display0) |
| WiFi        | TI WiLink wl12xx over SDIO (wl12xx_sdio) |
| Zigbee      | IEEE 802.15.4 backplate (wpan0) |
| Storage     | 256 MB NAND, JFFS2, A/B slots (see flash-layout.md) |
| Bootloader  | TI x-loader (MLO) + U-Boot 2012.10 "diamond board" (OMAP3 SPL) |
| Boot        | NAND A/B (kernel @0x400000 / @0x3A00000) or SD (fatload mmc0 uImage) |
| Console     | **ttyO0 @ 115200 8N1** (UART0) — confirmed from stock u-boot env |
| Stock kernel| 2.6.37 vendor BSP, glibc 2.19, BusyBox 1.19.3 |

OMAP3 has strong mainline support (same family as BeagleBoard), so the SoC,
UART, NAND, MMC, I2C, McSPI and MUSB USB come largely for free. The custom work
is the board device tree, the SPI display panel, the 802.15.4 backplate, and
wl12xx firmware/regulatory bring-up.

## Recovery / load path: OMAP3 USB peripheral boot ("DFU")

The OMAP3 is a **GP device** (x-loader string "TI OMAP SPL With GP CH"), so the
mask-ROM accepts **unsigned** images over USB peripheral boot. This is the
"DFU"-style recovery everyone refers to, and it is the key enabler:
- Load a first stage over USB with `omap_loader` / `usb-boot` (omap-u-boot-utils),
  which hands off to our u-boot; u-boot then loads a kernel (ymodem/DFU/tftp) or
  from NAND. No UART needed to *load*, and the board is effectively unbrickable.
- UART (ttyO0) remains valuable for *visibility* while debugging a kernel that
  boots but doesn't reach SSH. USB boot removes the brick risk; UART removes the
  blind spot. Pursue both; USB boot first.
- Live device currently exposes a `g_file_storage` USB gadget (vendor 0x2464);
  peripheral boot is a separate ROM mode (TI 0x0451:0xd00x) entered when NAND
  boot is forced to fall through.

## Remote bring-up path: kexec (hands-free, no physical access)

USB peripheral boot still needs one physical trigger (seat-on-base + hold), so it
is the unbrick safety net, not the day-to-day iteration channel. For hands-free
kernel testing we use **kexec** from the running stock Linux instead:
- The stock 2.6.37 kernel has `CONFIG_KEXEC=y`, and `/sys/kernel/kexec_loaded`
  works. A static-musl `kexec-tools` (armv7 hard-float) cross-built with nixpkgs
  (`nix build nixpkgs#pkgsCross.armv7l-hf-multiplatform.pkgsStatic.kexec-tools`)
  runs on the device after being pushed to `/media/scratch` (the `/tmp` noexec /
  no-sftp constraints apply — see tools/kexec/README.md).
- **PROVEN (2026-10-26, non-destructive):** pushed `kexec` + the stock zImage
  (stripped from `mtd6_boot0` uImage) and staged it with
  `kexec -l <zImage> --type=zImage --command-line="$(cat /proc/cmdline)" --atags`
  → rc=0, `kexec_loaded` 0→1, then `kexec -u` → back to 0. The full staging chain
  works over SSH; only the live jump `kexec -e` remains, and that is done only
  with someone physically present (a failed kexec hangs → needs a power-cycle).
- This makes modern-kernel iteration hands-free and reversible: a bad kernel is
  undone by a plain power-cycle (NAND still boots stock); nothing is flashed.
- A stable shell is required (the `nlsleepd` sleep daemon drops SSH on idle);
  `kill -STOP nlsleepd` holds it, `kill -CONT` restores normal sleep afterwards.

## Phases

0. **Foundation (done):** kas config, `meta-j49` layer, `nest-j49` machine
   (cortexa8hf-neon, systemd), hardware reference (docs/hw), docs. Yocto
   release: **blacksail** (bitbake 2.20 + oe-core + meta-yocto + meta-oe; the
   poky combo repo is not branched after walnascar).
1. **Boot path (kexec channel DONE, U-Boot TODO):**
   - USB peripheral boot PROVEN (2026-10-26): DFU (0451:d00e) via
     seat-on-base + hold-display; tools/omap-usbboot reads the ASIC ID without
     sudo. This is the UART-free load/recovery channel.
   - Remote kexec staging + live jumps PROVEN (2026-10-26/27): appended DTB
     (zImage header `_magic_end` bumped so kexec-tools loads it), post-mortem
     via ramoops + a DEBUG_LL DRAM ring at 0x83e00000, stock returns by warm
     reboot. ~57 jumps so far; see tools/kernel-firstlight/README.md.
   - TODO: modern U-Boot SPL/MLO for "diamond" pushed over USB boot (RAM only
     first), then NAND A/B boot with stock as fallback.
2. **J49 device tree + kernel (BOOTS, 2026-10-27..31):** mainline 6.6.64 runs
   to userspace on the hardware with `omap3-nest-j49.dts` + four patches, now
   packaged as `meta-j49/recipes-kernel/linux/linux-j49_6.6.bb`. Working:
   TWL4030/TPS65921 PMIC (I2C1, needs sys_clkout1 kept on), MADC, RTC, USB
   gadget console, GPIO state inherited from stock (`ti,no-reset-on-init`),
   WL1271 WiFi on MMC2 (26 MHz XTAL, WPA2 + DHCP + SSH), DSS DPI + S6D05A1
   panel + LM3530 backlight (full 320 px circle usable), backplate UART on
   ttyS2 with the type-0xa1 keepalive that defeats the ~29 min HU watchdog.
   NAND: partition table is in the DT (disabled) but the stock BCH4 ECC format
   (raw syndromes at the end of the OOB) is incompatible with mainline "bch4";
   needs a compat ECC variant in omap2.c (see docs/flash-layout.md).
   TODO: pinmux for the rest, gpio hogs for
   WiFi power (gpio103) / backlight enable (gpio177), ADBS-A330 ring sensor,
   EM357 zigbee, suspend/wakeup.
3. **Rootfs on the device:** first Yocto build of `j49-image` in progress.
   Iterate via kexec + initramfs/NFS over WiFi, then install into the NAND
   B slot (root1, JFFS2). 64 MB RAM: verify systemd fits, else fall back to
   a lighter init.
4. **Flash & A/B:** boot1/root1 with our image, stock A slot untouched.

## Open questions / risks
- SoC family CONFIRMED OMAP36xx/DM37xx-AM37xx via live CONTROL_IDCODE
  0x2B89102F (HAWKEYE 0xB891), ES1.2. Exact variant within the family (AM3703 vs
  DM3730 — affects whether SGX/DSP is present) still open; AM3703 (no SGX/DSP) is
  most likely given the SPI panel and absence of GPU use.
- Physical debug UART: it is UART0 / ttyO0 @ 115200 8N1 (logical, confirmed).
  Still need to locate the TX/RX/GND test points on the diamond PCB and wire a
  3.3V USB-TTL adapter. Do NOT feed 5V — OMAP3 UART is 1.8V/3.3V logic.
- Panel init sequence for s6d05a1/tianma — extract from the vendor 2.6.37 kernel
  (boot0 backup) when porting a mainline DRM/tinydrm panel driver.
- NAND timings/ECC for u-boot/DT (OMAP3 BCH vs Hamming).
- Hard-float ABI: set machine DEFAULTTUNE = "cortexa8hf-neon" to match stock armhf.

## Resolved
- RAM 64 MB; SDRAM @0x80000000; loadaddr 0x81000000.
- Console ttyO0 @ 115200.
- Board codename "diamond"; stock u-boot 2012.10.
- **GP device** — unsigned USB peripheral boot works (recovery/load path).
- SD boot exists in u-boot env BUT no SD slot/device populated (mmc0 controller
  present, no mmcblk) — so USB peripheral boot, not SD, is the dev/recovery path.
- A/B boot offsets and rootfs slots (see flash-layout.md).
- Full GPIO map, pinmux, kernel config, clocks captured (docs/hw/).
- Peripheral→driver mapping: WL1271 (SDIO/wlcore), S6D05A1 panel (SPI+DSS),
  LM3530 backlight (I2C), ADBS-A330 ring (SPI), EM357 zigbee (NCP, out-of-tree),
  STM32 backplate (serial). See docs/hw/README.md.

## Toolchain note (NixOS)
Host is NixOS 26.05. Do not use pip/.venv (fragile on Nix). Use the flake dev
shell, which provides `kas` + `kas-container`, and build inside the official
Yocto container (docker daemon running, user in the `docker` group):
```
nix develop
kas-container build kas/nest-j49.yml
```
kas-container keeps the Yocto build in a standard FHS container, sidestepping
NixOS path assumptions. The old `.venv` is obsolete and git-ignored.
